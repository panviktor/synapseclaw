import { useState, useEffect, useRef, useCallback } from 'react';
import { useNavigate, useSearchParams } from 'react-router-dom';
import {
  Send,
  Bot,
  User,
  AlertCircle,
  Copy,
  Check,
  Square,
  MoreVertical,
  Plus,
  Eraser,
  BrainCircuit,
  PanelLeftOpen,
  PanelRightOpen,
  Orbit,
  Sparkles,
} from 'lucide-react';
import type {
  WsMessage,
  ChatSessionInfo,
  ChatMessageInfo,
  StatusResponse,
  MemoryStatsResponse,
  ContextBudgetResponse,
  MemoryProjectionsResponse,
  PostTurnReportEvent,
} from '@/types/api';
import { WebSocketClient } from '@/lib/ws';
import { generateUUID } from '@/lib/uuid';
import {
  getStatus,
  getAgentStatus,
  putSummaryModel,
  getAgents,
  deleteChannelSession,
  getMemoryStats,
  getContextBudget,
  getMemoryProjections,
  type AgentEntry,
} from '@/lib/api';
import SessionSidebar from '@/components/chat/SessionSidebar';
import AgentRail from '@/components/chat/AgentRail';
import MemoryPulse from '@/components/chat/MemoryPulse';
import { useSSE } from '@/hooks/useSSE';
import {
  getCachedMessages,
  setCachedMessages,
  appendCachedMessage,
  deleteCachedSession,
  getSessionDraft,
  setSessionDraft,
  clearSessionDraft,
} from '@/hooks/useChatStore';

interface ChatMessage {
  id: string;
  role: 'user' | 'agent';
  content: string;
  timestamp: Date;
  event_type?: string;
}

function toCache(msg: ChatMessage) {
  return { id: msg.id, role: msg.role, content: msg.content, timestamp: msg.timestamp.getTime(), event_type: msg.event_type };
}

function fromCache(msg: { id: string; role: 'user' | 'agent'; content: string; timestamp: number; event_type?: string }): ChatMessage {
  return { id: msg.id, role: msg.role, content: msg.content, timestamp: new Date(msg.timestamp), event_type: msg.event_type };
}

export default function AgentChat() {
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const activeSession = searchParams.get('session');

  const [sessions, setSessions] = useState<ChatSessionInfo[]>([]);
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [input, setInput] = useState('');
  const [typing, setTyping] = useState(false);
  const [connected, setConnected] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [reconnecting, setReconnecting] = useState(false);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const [loading, setLoading] = useState(true);
  const [status, setStatus] = useState<StatusResponse | null>(null);
  const [agents, setAgents] = useState<AgentEntry[]>([]);
  const [activeAgent, setActiveAgent] = useState<string | null>(
    () => localStorage.getItem('synapseclaw_active_agent') || null,
  );
  const [memoryStats, setMemoryStats] = useState<MemoryStatsResponse | null>(null);
  const [contextBudget, setContextBudget] = useState<ContextBudgetResponse | null>(null);
  const [memoryProjections, setMemoryProjections] = useState<MemoryProjectionsResponse | null>(null);
  const [memoryPulseOpen, setMemoryPulseOpen] = useState(false);
  const [sessionSidebarOpen, setSessionSidebarOpen] = useState(false);
  const [localSessionCount, setLocalSessionCount] = useState(0);
  const [localHasActiveRun, setLocalHasActiveRun] = useState(false);
  const { events: learningEvents } = useSSE({
    filterTypes: ['post_turn_report'],
    maxEvents: 50,
  });

  const wsRef = useRef<WebSocketClient | null>(null);
  const messagesEndRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const [copiedId, setCopiedId] = useState<string | null>(null);
  const [actionsOpen, setActionsOpen] = useState(false);
  const [expandedMsgIds, setExpandedMsgIds] = useState<Set<string>>(new Set());
  const pendingContentRef = useRef('');
  const activeSessionRef = useRef(activeSession);
  activeSessionRef.current = activeSession;

  useEffect(() => {
    if (!activeSession) {
      const stored = localStorage.getItem('synapseclaw_active_session');
      if (stored) {
        setSearchParams({ session: stored }, { replace: true });
      }
    }
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (activeSession) {
      localStorage.setItem('synapseclaw_active_session', activeSession);
    }
  }, [activeSession]);

  const loadMemorySurface = useCallback(async (agentId: string | null) => {
    try {
      const [statsResponse, budgetResponse, projectionsResponse] = await Promise.all([
        getMemoryStats(agentId),
        getContextBudget(agentId),
        getMemoryProjections(agentId, 6),
      ]);
      setMemoryStats(statsResponse);
      setContextBudget(budgetResponse);
      setMemoryProjections(projectionsResponse);
    } catch {
      setMemoryStats(null);
      setContextBudget(null);
      setMemoryProjections(null);
    }
  }, []);

  const agentFromUrl = searchParams.get('agent');
  useEffect(() => {
    if (agentFromUrl && agentFromUrl !== activeAgent) {
      setActiveAgent(agentFromUrl);
      localStorage.setItem('synapseclaw_active_agent', agentFromUrl);
    }
  }, [agentFromUrl]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (activeSession) {
      setInput(getSessionDraft(activeSession));
    } else {
      setInput('');
    }
  }, [activeSession]);

  useEffect(() => {
    if (activeSession) {
      setSessionDraft(activeSession, input);
    }
  }, [input, activeSession]);

  useEffect(() => {
    if (!activeSession) return;
    const cached = getCachedMessages(activeSession);
    if (cached) {
      setMessages(cached.map(fromCache));
    }
  }, [activeSession]);

  useEffect(() => {
    loadMemorySurface(activeAgent);
  }, [activeAgent, loadMemorySurface]);

  useEffect(() => {
    if (activeAgent !== null) return;
    setLocalSessionCount(sessions.length);
    setLocalHasActiveRun(typing || sessions.some((session) => session.has_active_run));
  }, [activeAgent, sessions, typing]);

  useEffect(() => {
    const ws = new WebSocketClient({ agent: activeAgent });

    ws.onOpen = () => {
      setConnected(true);
      setError(null);
      setReconnecting(false);
      if (activeAgent) {
        getAgentStatus(activeAgent).then(setStatus).catch(() => setStatus(null));
      } else {
        getStatus().then(setStatus).catch(() => {});
      }
      getAgents().then(setAgents).catch(() => {});
      ws.rpc<{ sessions: ChatSessionInfo[] }>('sessions.list')
        .then((res) => {
          setSessions(res.sessions);
          if (!activeSessionRef.current && res.sessions.length > 0) {
            const first = res.sessions[0]!.key;
            setSearchParams({ session: first }, { replace: true });
            loadHistory(ws, first);
          } else if (activeSessionRef.current) {
            loadHistory(ws, activeSessionRef.current);
          } else {
            setLoading(false);
          }
        })
        .catch(() => setLoading(false));
    };

    ws.onClose = () => {
      setConnected(false);
      setReconnecting(true);
    };

    ws.onError = () => {
      setError('Connection error. Attempting to reconnect...');
      setReconnecting(true);
    };

    ws.onMessage = (msg: WsMessage) => {
      switch (msg.type) {
        case 'chunk':
          pendingContentRef.current += msg.content ?? '';
          break;

        case 'error': {
          const chatMsg: ChatMessage = {
            id: generateUUID(),
            role: 'agent',
            content: `[Error] ${msg.message ?? 'Unknown error'}`,
            timestamp: new Date(),
            event_type: 'error',
          };
          setMessages((prev) => [...prev, chatMsg]);
          if (activeSessionRef.current) {
            appendCachedMessage(activeSessionRef.current, toCache(chatMsg));
          }
          setTyping(false);
          pendingContentRef.current = '';
          break;
        }

        case 'tool_call': {
          if (msg.session_key && msg.session_key !== activeSessionRef.current) break;
          const toolCallMsg: ChatMessage = {
            id: generateUUID(),
            role: 'agent',
            content: msg.content ?? '',
            timestamp: new Date(),
            event_type: 'tool_call',
          };
          setMessages((prev) => [...prev, toolCallMsg]);
          if (msg.session_key) {
            appendCachedMessage(msg.session_key, toCache(toolCallMsg));
          }
          break;
        }

        case 'tool_result': {
          if (msg.session_key && msg.session_key !== activeSessionRef.current) break;
          const toolResultMsg: ChatMessage = {
            id: generateUUID(),
            role: 'agent',
            content: msg.content ?? '',
            timestamp: new Date(),
            event_type: 'tool_result',
          };
          setMessages((prev) => [...prev, toolResultMsg]);
          if (msg.session_key) {
            appendCachedMessage(msg.session_key, toCache(toolResultMsg));
          }
          break;
        }

        case 'assistant': {
          if (msg.session_key && msg.session_key !== activeSessionRef.current) break;
          const assistantMsg: ChatMessage = {
            id: generateUUID(),
            role: 'agent',
            content: msg.content ?? '',
            timestamp: new Date(),
            event_type: 'assistant',
          };
          setMessages((prev) => [...prev, assistantMsg]);
          if (msg.session_key) {
            appendCachedMessage(msg.session_key, toCache(assistantMsg));
          }
          setTyping(false);
          pendingContentRef.current = '';
          break;
        }

        default:
          if (msg.type === 'session.updated' || msg.type === 'session.deleted') {
            ws.rpc<{ sessions: ChatSessionInfo[] }>('sessions.list')
              .then((r) => setSessions(r.sessions))
              .catch(() => {});
          }
          if (msg.type === 'session.run_started') {
            if (msg.session_key && msg.session_key === activeSessionRef.current) {
              setTyping(true);
            }
          }
          if (msg.type === 'session.run_finished' || msg.type === 'session.run_interrupted') {
            if (msg.session_key && msg.session_key === activeSessionRef.current) {
              setTyping(false);
              loadHistory(ws, activeSessionRef.current);
            }
            ws.rpc<{ sessions: ChatSessionInfo[] }>('sessions.list')
              .then((r) => setSessions(r.sessions))
              .catch(() => {});
          }
          break;
      }
    };

    ws.connect();
    wsRef.current = ws;

    return () => {
      ws.disconnect();
    };
  }, [activeAgent]); // eslint-disable-line react-hooks/exhaustive-deps

  const loadHistory = useCallback(async (ws: WebSocketClient, sessionKey: string) => {
    const cached = getCachedMessages(sessionKey);
    if (cached && cached.length > 0) {
      setMessages(cached.map(fromCache));
      setLoading(false);
    } else {
      setLoading(true);
    }

    try {
      const res = await ws.rpc<{
        messages: ChatMessageInfo[];
        session_key: string;
        label: string | null;
        session_summary: string | null;
        current_goal: string | null;
      }>('chat.history', { session: sessionKey, limit: 50 });

      const mapped: ChatMessage[] = res.messages.map((m) => ({
        id: String(m.id),
        role: (m.role === 'user' ? 'user' : 'agent') as 'user' | 'agent',
        content: m.content,
        timestamp: new Date(m.timestamp * 1000),
        event_type: m.event_type,
      }));
      setMessages(mapped);
      setCachedMessages(
        sessionKey,
        mapped.map(toCache),
        { sessionSummary: res.session_summary ?? undefined, currentGoal: res.current_goal ?? undefined },
      );
    } catch {
      if (!cached || cached.length === 0) {
        setMessages([]);
      }
    }
    setLoading(false);
  }, []);

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages, typing]);

  const handleSelectSession = useCallback(
    (key: string) => {
      if (key === activeSession) return;
      setSessionSidebarOpen(false);
      setSearchParams({ session: key }, { replace: true });
      if (wsRef.current?.connected) {
        loadHistory(wsRef.current, key);
      }
    },
    [activeSession, setSearchParams, loadHistory],
  );

  const handleNewSession = useCallback(async () => {
    if (!wsRef.current?.connected) return;
    try {
      const res = await wsRef.current.rpc<{ session_key: string }>('sessions.new');
      const listRes = await wsRef.current.rpc<{ sessions: ChatSessionInfo[] }>('sessions.list');
      setSessions(listRes.sessions);
      setSearchParams({ session: res.session_key }, { replace: true });
      setMessages([]);
      setSessionSidebarOpen(false);
    } catch {
      // ignore
    }
  }, [setSearchParams]);

  const handleRenameSession = useCallback(async (key: string, label: string) => {
    if (!wsRef.current?.connected) return;
    try {
      await wsRef.current.rpc('sessions.rename', { key, label });
      setSessions((prev) =>
        prev.map((s) => (s.key === key ? { ...s, label } : s)),
      );
    } catch {
      // ignore
    }
  }, []);

  const handleDeleteSession = useCallback(
    async (key: string) => {
      if (!wsRef.current?.connected) return;
      try {
        await wsRef.current.rpc('sessions.delete', { key });
        deleteCachedSession(key);
        setSessions((prev) => {
          const remaining = prev.filter((s) => s.key !== key);
          if (key === activeSession && remaining.length > 0) {
            const next = remaining[0]!.key;
            setSearchParams({ session: next }, { replace: true });
            if (wsRef.current?.connected) {
              loadHistory(wsRef.current, next);
            }
          } else if (remaining.length === 0) {
            setSearchParams({}, { replace: true });
            setMessages([]);
          }
          return remaining;
        });
      } catch {
        // ignore
      }
    },
    [activeSession, setSearchParams, loadHistory],
  );

  const handleAgentChange = useCallback(
    (agentId: string | null) => {
      const newAgent = agentId || null;
      setActiveAgent(newAgent);
      setMemoryPulseOpen(false);
      setSessionSidebarOpen(false);
      if (newAgent) {
        localStorage.setItem('synapseclaw_active_agent', newAgent);
      } else {
        localStorage.removeItem('synapseclaw_active_agent');
      }
      setMessages([]);
      setSessions([]);
      setSearchParams({}, { replace: true });
    },
    [setSearchParams],
  );

  const handleSummaryModelChange = useCallback(async (model: string | null) => {
    try {
      const res = await putSummaryModel(model, activeAgent);
      setStatus((prev) => prev ? { ...prev, summary_model: res.summary_model } : prev);
    } catch {
      // ignore
    }
  }, [activeAgent]);

  const handleClearHistory = useCallback(async () => {
    if (!wsRef.current?.connected || !activeSession) return;
    try {
      await wsRef.current.rpc('sessions.reset', { key: activeSession });
      setMessages([]);
      deleteCachedSession(activeSession);
    } catch {
      // ignore
    }
  }, [activeSession]);

  const handleSend = async () => {
    const trimmed = input.trim();
    if (!trimmed || !wsRef.current?.connected) return;

    let sendSession = activeSession;
    if (!sendSession) {
      try {
        const res = await wsRef.current.rpc<{ session_key: string }>('sessions.new');
        sendSession = res.session_key;
        setSearchParams({ session: sendSession }, { replace: true });
        const listRes = await wsRef.current.rpc<{ sessions: ChatSessionInfo[] }>('sessions.list');
        setSessions(listRes.sessions);
        setSessionSidebarOpen(false);
      } catch {
        return;
      }
    }

    const chatMsg: ChatMessage = {
      id: generateUUID(),
      role: 'user',
      content: trimmed,
      timestamp: new Date(),
      event_type: 'user',
    };
    setMessages((prev) => [...prev, chatMsg]);
    appendCachedMessage(sendSession, toCache(chatMsg));

    setTyping(true);
    pendingContentRef.current = '';

    wsRef.current.rpc<{ run_id: string; response?: string; aborted?: boolean }>(
      'chat.send',
      { session: sendSession, message: trimmed },
      300000,
    ).then((res) => {
      if (res.aborted) {
        const abortMsg: ChatMessage = {
          id: generateUUID(),
          role: 'agent',
          content: '[Generation aborted]',
          timestamp: new Date(),
          event_type: 'interrupted',
        };
        if (activeSessionRef.current === sendSession) {
          setMessages((prev) => [...prev, abortMsg]);
        }
      }
      if (activeSessionRef.current === sendSession) {
        setTyping(false);
      }
      wsRef.current?.rpc<{ sessions: ChatSessionInfo[] }>('sessions.list')
        .then((r) => setSessions(r.sessions))
        .catch(() => {});
    }).catch((err) => {
      const isTimeout = typeof err?.message === 'string' && err.message.includes('RPC timeout');
      if (isTimeout) {
        return;
      }
      const errorMsg: ChatMessage = {
        id: generateUUID(),
        role: 'agent',
        content: `[Error] ${err.message ?? 'Unknown error'}`,
        timestamp: new Date(),
        event_type: 'error',
      };
      if (activeSessionRef.current === sendSession) {
        setMessages((prev) => [...prev, errorMsg]);
      }
      appendCachedMessage(sendSession, toCache(errorMsg));
      if (activeSessionRef.current === sendSession) {
        setTyping(false);
      }
    });

    setInput('');
    if (activeSession) clearSessionDraft(activeSession);
    if (inputRef.current) {
      inputRef.current.style.height = 'auto';
      inputRef.current.focus();
    }
  };

  const handleAbort = useCallback(async () => {
    if (!wsRef.current?.connected) return;
    try {
      await wsRef.current.rpc('chat.abort', { session: activeSession });
      setTyping(false);
      pendingContentRef.current = '';
    } catch {
      // ignore
    }
  }, [activeSession]);

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  };

  const handleTextareaChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    setInput(e.target.value);
    e.target.style.height = 'auto';
    e.target.style.height = `${Math.min(e.target.scrollHeight, 200)}px`;
  };

  const handleCopy = useCallback((msgId: string, content: string) => {
    navigator.clipboard.writeText(content).then(() => {
      setCopiedId(msgId);
      setTimeout(() => setCopiedId((prev) => (prev === msgId ? null : prev)), 2000);
    });
  }, []);

  const selectedSession = sessions.find((s) => s.key === activeSession) ?? null;
  const effectiveAgentId = activeAgent ?? memoryStats?.agent_id ?? null;
  const latestLearningEvent =
    effectiveAgentId === null
      ? null
      : [...learningEvents]
          .reverse()
          .find(
            (event): event is PostTurnReportEvent =>
              event.type === 'post_turn_report' &&
              typeof event.agent_id === 'string' &&
              event.agent_id === effectiveAgentId,
          ) ?? null;
  const activeAgentInfo = activeAgent ? agents.find((agent) => agent.agent_id === activeAgent) ?? null : null;
  const activeAgentLabel = activeAgentInfo?.agent_id ?? activeAgent ?? 'Local Runtime';
  const sessionTitle =
    selectedSession?.label ??
    (selectedSession?.kind === 'channel'
      ? selectedSession.key
      : selectedSession?.kind === 'ipc'
        ? 'IPC Session'
        : activeSession
          ? 'Session'
          : 'Fresh Canvas');
  const sessionMeta = selectedSession?.kind === 'channel'
    ? `${selectedSession.channel ?? 'channel'} · read only`
    : selectedSession?.kind === 'ipc'
      ? 'ipc routed session'
      : `${selectedSession?.message_count ?? 0} messages`;

  useEffect(() => {
    if (!latestLearningEvent) return;
    loadMemorySurface(activeAgent);
  }, [activeAgent, latestLearningEvent?.agent_id, latestLearningEvent?.timestamp, loadMemorySurface]);

  return (
    <div className="flex h-[calc(100vh-3.5rem)] bg-[radial-gradient(circle_at_top_left,var(--glow-primary),transparent_30%),var(--bg-primary)]">
      {sessionSidebarOpen && (
        <button
          type="button"
          className="fixed inset-0 z-30 bg-black/25 backdrop-blur-[1px] lg:hidden"
          onClick={() => setSessionSidebarOpen(false)}
          aria-label="Close sessions"
        />
      )}

      <SessionSidebar
        sessions={sessions}
        activeKey={activeSession}
        collapsed={sidebarCollapsed}
        mobileOpen={sessionSidebarOpen}
        status={status}
        agents={agents}
        activeAgent={activeAgent}
        onToggle={() => setSidebarCollapsed(!sidebarCollapsed)}
        onMobileClose={() => setSessionSidebarOpen(false)}
        onSelect={handleSelectSession}
        onNew={handleNewSession}
        onRename={handleRenameSession}
        onDelete={(key) => {
          const session = sessions.find((s) => s.key === key);
          if (session?.kind === 'channel') {
            deleteChannelSession(key).then(() => {
              setSessions((prev) => prev.filter((s) => s.key !== key));
              if (activeSession === key) {
                setSearchParams({}, { replace: true });
                setMessages([]);
              }
            }).catch(() => {});
          } else {
            handleDeleteSession(key);
          }
        }}
        onSummaryModelChange={handleSummaryModelChange}
        onAgentChange={handleAgentChange}
      />

      <div className="flex min-w-0 flex-1 flex-col">
        {error && (
          <div className="flex items-center gap-2 border-b border-[#C73E3E]/20 bg-[var(--status-error)]/10 px-4 py-2 text-sm text-[#C73E3E] animate-fade-in">
            <AlertCircle className="h-4 w-4 flex-shrink-0" />
            {error}
          </div>
        )}

        <div className="border-b border-[var(--border-default)] bg-[linear-gradient(180deg,var(--bg-secondary),rgba(255,255,255,0))]">
          <div className="space-y-4 px-4 py-4 md:px-5 animate-panel-reveal">
            <div className="flex flex-col gap-3 xl:flex-row xl:items-start xl:justify-between">
              <div>
                <p className="text-[11px] font-semibold uppercase tracking-[0.28em] text-[var(--text-placeholder)]">
                  Agent Workbench
                </p>
                <div className="mt-2 flex flex-wrap items-center gap-2">
                  <h1 className="text-2xl font-semibold tracking-tight text-[var(--text-primary)]">
                    {activeAgentLabel}
                  </h1>
                  {activeAgentInfo?.role && (
                    <span className="rounded-full border border-[var(--border-default)] bg-[var(--bg-card)] px-2.5 py-1 text-[10px] font-semibold uppercase tracking-wide text-[var(--text-muted)]">
                      {activeAgentInfo.role}
                    </span>
                  )}
                  <span className={`rounded-full px-2.5 py-1 text-[10px] font-semibold uppercase tracking-wide ${
                    connected
                      ? 'bg-[var(--status-success)]/12 text-[var(--status-success)]'
                      : 'bg-[var(--status-error)]/12 text-[var(--status-error)]'
                  }`}>
                    {connected ? 'online' : reconnecting ? 'reconnecting' : 'offline'}
                  </span>
                </div>
                <div className="mt-3 flex flex-wrap gap-2 text-xs text-[var(--text-muted)]">
                  <span className="rounded-full border border-[var(--border-default)] bg-[var(--bg-card)] px-2.5 py-1">
                    {status?.model ?? 'model pending'}
                  </span>
                  <span className="rounded-full border border-[var(--border-default)] bg-[var(--bg-card)] px-2.5 py-1">
                    {sessions.length} sessions
                  </span>
                  <span className="rounded-full border border-[var(--border-default)] bg-[var(--bg-card)] px-2.5 py-1">
                    {memoryStats ? `${memoryStats.total_entries} memory entries` : 'memory loading'}
                  </span>
                </div>
              </div>

              <div className="flex flex-wrap gap-2">
                <button
                  onClick={() => setSessionSidebarOpen(true)}
                  className="btn-secondary inline-flex items-center gap-2 px-4 py-2 text-sm lg:hidden"
                >
                  <PanelLeftOpen className="h-4 w-4" />
                  Sessions
                </button>
                <button
                  onClick={handleNewSession}
                  className="btn-primary inline-flex items-center gap-2 px-4 py-2 text-sm font-medium"
                >
                  <Plus className="h-4 w-4" />
                  New Chat
                </button>
                <button
                  onClick={() => navigate(activeAgent ? `/memory?agent=${encodeURIComponent(activeAgent)}` : '/memory')}
                  className="btn-secondary inline-flex items-center gap-2 px-4 py-2 text-sm"
                >
                  <BrainCircuit className="h-4 w-4" />
                  Memory Studio
                </button>
                <button
                  onClick={() => setMemoryPulseOpen(true)}
                  className="btn-secondary inline-flex items-center gap-2 px-4 py-2 text-sm xl:hidden"
                >
                  <PanelRightOpen className="h-4 w-4" />
                  Pulse
                </button>
              </div>
            </div>

            <AgentRail
              agents={agents}
              activeAgent={activeAgent}
              connected={connected}
              typing={typing}
              localSessionCount={localSessionCount}
              localHasActiveRun={localHasActiveRun}
              onSelect={handleAgentChange}
            />
          </div>
        </div>

        <div className="flex min-h-0 flex-1">
          <div className="flex min-w-0 flex-1 flex-col">
            <div className="border-b border-[var(--border-default)] bg-[var(--bg-card)]/70 px-4 py-3 backdrop-blur md:px-5">
              <div
                key={`${activeAgentLabel}:${activeSession ?? 'fresh'}`}
                className="animate-scope-shift flex flex-col gap-3 md:flex-row md:items-center md:justify-between"
              >
                <div className="flex items-center gap-2">
                  <span className="inline-flex h-8 w-8 items-center justify-center rounded-2xl bg-[var(--glow-secondary)] text-[var(--accent-primary)]">
                    <Orbit className="h-4 w-4" />
                  </span>
                  <div>
                    <p className="text-sm font-semibold text-[var(--text-primary)]">{sessionTitle}</p>
                    <p className="text-xs text-[var(--text-muted)]">{sessionMeta}</p>
                  </div>
                </div>
                <div className="flex flex-wrap gap-2 text-xs">
                  {selectedSession?.current_goal && (
                    <span className="rounded-full border border-[var(--border-default)] bg-[var(--bg-secondary)] px-2.5 py-1 text-[var(--text-secondary)]">
                      goal: {selectedSession.current_goal}
                    </span>
                  )}
                  {latestLearningEvent && (
                    <span className="rounded-full bg-[var(--accent-primary)]/10 px-2.5 py-1 text-[var(--accent-primary)]">
                      <span className="inline-flex items-center gap-1">
                        <Sparkles className="h-3 w-3" />
                        {latestLearningEvent.signal}
                      </span>
                    </span>
                  )}
                </div>
              </div>
            </div>

            <div className="flex-1 space-y-4 overflow-y-auto p-4 md:p-5">
              {loading ? (
                <div className="flex h-full items-center justify-center">
                  <div className="h-8 w-8 rounded-full border-2 border-[var(--accent-primary)]/20 border-t-[#D95A1E] animate-spin" />
                </div>
              ) : messages.length === 0 ? (
                <div className="flex h-full flex-col items-center justify-center text-[var(--text-muted)] animate-fade-in">
                  <div
                    className="mb-4 flex h-16 w-16 items-center justify-center rounded-3xl"
                    style={{ background: 'var(--glow-secondary)' }}
                  >
                    <Bot className="h-8 w-8 text-[var(--accent-primary)]" />
                  </div>
                  <p className="mb-1 text-lg font-semibold text-[var(--text-primary)]">SynapseClaw Workbench</p>
                  <p className="text-sm text-[var(--text-muted)]">Start a session and watch memory evolve in the side rail.</p>
                </div>
              ) : (
                messages.map((msg, idx) => (
                  <div
                    key={msg.id}
                    className={`group flex items-start gap-3 ${
                      msg.role === 'user'
                        ? 'flex-row-reverse animate-slide-in-right'
                        : 'animate-slide-in-left'
                    }`}
                    style={{ animationDelay: `${Math.min(idx * 30, 200)}ms` }}
                  >
                    <div
                      className="flex h-8 w-8 flex-shrink-0 items-center justify-center rounded-xl"
                      style={{
                        background:
                          msg.role === 'user'
                            ? 'var(--accent-primary)'
                            : 'var(--bg-secondary)',
                      }}
                    >
                      {msg.role === 'user' ? (
                        <User className="h-4 w-4 text-white" />
                      ) : (
                        <Bot className="h-4 w-4 text-[var(--accent-primary)]" />
                      )}
                    </div>
                    <div className="relative max-w-[75%]">
                      <div
                        className={`rounded-2xl px-4 py-3 ${
                          msg.role === 'user'
                            ? 'text-white'
                            : msg.event_type === 'tool_call' || msg.event_type === 'tool_result'
                              ? 'border border-[var(--border-default)] text-[var(--text-secondary)]'
                              : 'border border-[var(--border-default)] text-[var(--text-primary)]'
                        }`}
                        style={{
                          background:
                            msg.role === 'user'
                              ? 'var(--accent-primary)'
                              : msg.event_type === 'tool_call' || msg.event_type === 'tool_result'
                                ? 'var(--bg-primary)'
                                : 'var(--bg-card)',
                        }}
                      >
                        {msg.event_type === 'tool_call' && (
                          <p className="mb-1 text-[10px] uppercase tracking-wide text-[var(--text-muted)]">Tool Call</p>
                        )}
                        {msg.event_type === 'tool_result' && (
                          <p className="mb-1 text-[10px] uppercase tracking-wide text-[var(--text-muted)]">Tool Result</p>
                        )}
                        {(() => {
                          const isSystem = msg.event_type === 'tool_call' || msg.event_type === 'tool_result' || msg.event_type === 'error' || msg.event_type === 'interrupted';
                          const lines = msg.content.split('\n');
                          const isLong = isSystem && lines.length > 3;
                          const isExpanded = expandedMsgIds.has(msg.id);
                          return (
                            <>
                              <p className={`text-sm whitespace-pre-wrap break-words ${
                                isSystem ? 'font-mono text-xs' : ''
                              } ${isLong && !isExpanded ? 'line-clamp-3' : ''}`}>
                                {msg.content}
                              </p>
                              {isLong && (
                                <button
                                  onClick={(e) => {
                                    e.stopPropagation();
                                    setExpandedMsgIds((prev) => {
                                      const next = new Set(prev);
                                      if (next.has(msg.id)) next.delete(msg.id); else next.add(msg.id);
                                      return next;
                                    });
                                  }}
                                  className="mt-1 text-[10px] text-[var(--accent-primary)] hover:underline"
                                >
                                  {isExpanded ? 'Collapse' : `Show all (${lines.length} lines)`}
                                </button>
                              )}
                            </>
                          );
                        })()}
                        <p
                          className={`mt-1.5 text-[10px] ${
                            msg.role === 'user' ? 'text-white/70' : 'text-[var(--text-placeholder)]'
                          }`}
                        >
                          {msg.timestamp.toLocaleTimeString()}
                        </p>
                      </div>
                      <button
                        onClick={() => handleCopy(msg.id, msg.content)}
                        aria-label="Copy message"
                        className="absolute right-1 top-1 rounded-lg border border-[var(--border-default)] bg-[var(--bg-card)] p-1.5 text-[var(--text-muted)] opacity-0 transition-all duration-300 hover:border-[var(--accent-primary)]/30 hover:text-[var(--text-primary)] group-hover:opacity-100"
                      >
                        {copiedId === msg.id ? (
                          <Check className="h-3 w-3 text-[#2D8A4E]" />
                        ) : (
                          <Copy className="h-3 w-3" />
                        )}
                      </button>
                    </div>
                  </div>
                ))
              )}

              {typing && (
                <div className="flex items-start gap-3 animate-fade-in">
                  <div
                    className="flex h-8 w-8 flex-shrink-0 items-center justify-center rounded-xl"
                    style={{ background: 'var(--bg-secondary)' }}
                  >
                    <Bot className="h-4 w-4 text-[var(--accent-primary)]" />
                  </div>
                  <div
                    className="rounded-2xl border border-[var(--border-default)] px-4 py-3"
                    style={{ background: 'var(--bg-card)' }}
                  >
                    <div className="flex items-center gap-1.5">
                      <span
                        className="h-1.5 w-1.5 rounded-full bg-[#D95A1E] animate-bounce"
                        style={{ animationDelay: '0ms' }}
                      />
                      <span
                        className="h-1.5 w-1.5 rounded-full bg-[#D95A1E] animate-bounce"
                        style={{ animationDelay: '150ms' }}
                      />
                      <span
                        className="h-1.5 w-1.5 rounded-full bg-[#D95A1E] animate-bounce"
                        style={{ animationDelay: '300ms' }}
                      />
                    </div>
                  </div>
                </div>
              )}

              <div ref={messagesEndRef} />
            </div>

            {selectedSession?.kind === 'channel' ? (
              <div className="border-t border-[var(--border-default)] bg-[var(--bg-card)] px-4 py-3 text-center">
                <p className="text-xs text-[var(--text-muted)]">
                  Read-only conversation from <strong>{selectedSession.channel ?? 'channel'}</strong>. Reply through the channel client.
                </p>
              </div>
            ) : (
              <div className="border-t border-[var(--border-default)] bg-[var(--bg-card)] p-4">
                <div className="mx-auto flex max-w-4xl items-end gap-3">
                  <div className="relative flex-shrink-0">
                    <button
                      onClick={() => setActionsOpen(!actionsOpen)}
                      className="rounded-xl p-3 text-[var(--text-muted)] transition-colors hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)]"
                      title="Actions"
                    >
                      <MoreVertical className="h-5 w-5" />
                    </button>
                    {actionsOpen && (
                      <>
                        <div className="fixed inset-0 z-10" onClick={() => setActionsOpen(false)} />
                        <div className="absolute bottom-full left-0 z-20 mb-2 w-44 overflow-hidden rounded-xl border border-[var(--border-default)] bg-[var(--bg-card)] shadow-lg">
                          <button
                            onClick={() => { setActionsOpen(false); handleNewSession(); }}
                            className="flex w-full items-center gap-2 px-3 py-2.5 text-left text-xs text-[var(--text-secondary)] transition-colors hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)]"
                          >
                            <Plus className="h-3.5 w-3.5" />
                            New Chat
                          </button>
                          <button
                            onClick={() => { setActionsOpen(false); handleClearHistory(); }}
                            disabled={!activeSession || messages.length === 0}
                            className="flex w-full items-center gap-2 px-3 py-2.5 text-left text-xs text-[var(--text-secondary)] transition-colors hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)] disabled:pointer-events-none disabled:opacity-30"
                          >
                            <Eraser className="h-3.5 w-3.5" />
                            Clear History
                          </button>
                        </div>
                      </>
                    )}
                  </div>
                  <div className="flex-1">
                    <textarea
                      ref={inputRef}
                      rows={1}
                      value={input}
                      onChange={handleTextareaChange}
                      onKeyDown={handleKeyDown}
                      placeholder={connected ? 'Type a message...' : 'Connecting...'}
                      disabled={!connected}
                      className="input-warm w-full resize-none overflow-y-auto px-4 py-3 text-sm disabled:opacity-40"
                      style={{ minHeight: '44px', maxHeight: '200px' }}
                    />
                  </div>
                  {typing ? (
                    <button
                      onClick={handleAbort}
                      className="flex-shrink-0 rounded-xl bg-[var(--status-error)] p-3 text-white transition-colors hover:bg-[var(--accent-primary-hover)]"
                      title="Stop generation"
                    >
                      <Square className="h-5 w-5" />
                    </button>
                  ) : (
                    <button
                      onClick={handleSend}
                      disabled={!connected || !input.trim()}
                      className="btn-primary flex-shrink-0 rounded-xl p-3"
                    >
                      <Send className="h-5 w-5" />
                    </button>
                  )}
                </div>
                <div className="mt-2 flex items-center justify-center gap-2">
                  <span
                    className={`inline-block h-1.5 w-1.5 rounded-full ${
                      connected ? 'bg-[var(--status-success)]' : reconnecting ? 'bg-[#C9872C] animate-pulse' : 'bg-[var(--status-error)]'
                    }`}
                  />
                  <span className="text-[10px] text-[var(--text-placeholder)]">
                    {connected ? 'Connected' : reconnecting ? 'Reconnecting...' : 'Disconnected'}
                  </span>
                </div>
              </div>
            )}
          </div>

          <div className="hidden xl:block xl:w-[360px] xl:border-l xl:border-[var(--border-default)] animate-panel-reveal">
            <MemoryPulse
              stats={memoryStats}
              budget={contextBudget}
              projections={memoryProjections}
              lastReport={latestLearningEvent}
              session={selectedSession}
              agentLabel={activeAgentLabel}
            />
          </div>
        </div>
      </div>

      {memoryPulseOpen && (
        <div className="fixed inset-0 z-50 xl:hidden">
          <button
            className="absolute inset-0 bg-black/30"
            onClick={() => setMemoryPulseOpen(false)}
            aria-label="Close memory pulse"
          />
          <div className="absolute inset-y-0 right-0 w-full max-w-sm animate-slide-in-right border-l border-[var(--border-default)] bg-[var(--bg-secondary)] shadow-2xl">
            <MemoryPulse
              stats={memoryStats}
              budget={contextBudget}
              projections={memoryProjections}
              lastReport={latestLearningEvent}
              session={selectedSession}
              agentLabel={activeAgentLabel}
              onClose={() => setMemoryPulseOpen(false)}
            />
          </div>
        </div>
      )}
    </div>
  );
}
