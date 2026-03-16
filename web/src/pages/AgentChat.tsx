import { useState, useEffect, useRef, useCallback } from 'react';
import { useSearchParams } from 'react-router-dom';
import { Send, Bot, User, AlertCircle, Copy, Check, Square } from 'lucide-react';
import type { WsMessage, ChatSessionInfo, ChatMessageInfo } from '@/types/api';
import { WebSocketClient } from '@/lib/ws';
import { generateUUID } from '@/lib/uuid';
import { useDraft } from '@/hooks/useDraft';
import SessionSidebar from '@/components/chat/SessionSidebar';

interface ChatMessage {
  id: string;
  role: 'user' | 'agent';
  content: string;
  timestamp: Date;
  kind?: string;
}

export default function AgentChat() {
  const [searchParams, setSearchParams] = useSearchParams();
  const activeSession = searchParams.get('session');

  const [sessions, setSessions] = useState<ChatSessionInfo[]>([]);
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [input, setInput] = useState('');
  const [typing, setTyping] = useState(false);
  const [connected, setConnected] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const [loading, setLoading] = useState(true);

  const wsRef = useRef<WebSocketClient | null>(null);
  const messagesEndRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const [copiedId, setCopiedId] = useState<string | null>(null);
  const pendingContentRef = useRef('');
  const activeSessionRef = useRef(activeSession);
  activeSessionRef.current = activeSession;

  // Draft keyed by session
  const draftKey = `agent-chat-${activeSession ?? 'default'}`;
  const { draft, saveDraft, clearDraft } = useDraft(draftKey);

  // Sync draft → input on session switch
  useEffect(() => {
    setInput(draft);
  }, [draftKey]); // eslint-disable-line react-hooks/exhaustive-deps

  // Save draft on input change
  useEffect(() => {
    saveDraft(input);
  }, [input, saveDraft]);

  // ── WebSocket setup ──────────────────────────────────────────────
  useEffect(() => {
    const ws = new WebSocketClient();

    ws.onOpen = () => {
      setConnected(true);
      setError(null);
      // Load sessions list
      ws.rpc<{ sessions: ChatSessionInfo[] }>('sessions.list')
        .then((res) => {
          setSessions(res.sessions);
          // If no active session in URL, use first or create default
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
    };

    ws.onError = () => {
      setError('Connection error. Attempting to reconnect...');
    };

    ws.onMessage = (msg: WsMessage) => {
      switch (msg.type) {
        case 'chunk':
          setTyping(true);
          pendingContentRef.current += msg.content ?? '';
          break;

        case 'message':
        case 'done': {
          const content = msg.full_response ?? msg.content ?? pendingContentRef.current;
          if (content) {
            setMessages((prev) => [
              ...prev,
              {
                id: generateUUID(),
                role: 'agent',
                content,
                timestamp: new Date(),
                kind: 'assistant',
              },
            ]);
          }
          pendingContentRef.current = '';
          setTyping(false);
          // Refresh sessions list for updated preview/counts
          ws.rpc<{ sessions: ChatSessionInfo[] }>('sessions.list')
            .then((res) => setSessions(res.sessions))
            .catch(() => {});
          break;
        }

        case 'tool_call':
          setMessages((prev) => [
            ...prev,
            {
              id: generateUUID(),
              role: 'agent',
              content: `[Tool Call] ${msg.name ?? 'unknown'}(${JSON.stringify(msg.args ?? {})})`,
              timestamp: new Date(),
              kind: 'tool_call',
            },
          ]);
          break;

        case 'tool_result':
          setMessages((prev) => [
            ...prev,
            {
              id: generateUUID(),
              role: 'agent',
              content: `[Tool Result] ${msg.output ?? ''}`,
              timestamp: new Date(),
              kind: 'tool_result',
            },
          ]);
          break;

        case 'error':
          setMessages((prev) => [
            ...prev,
            {
              id: generateUUID(),
              role: 'agent',
              content: `[Error] ${msg.message ?? 'Unknown error'}`,
              timestamp: new Date(),
              kind: 'error',
            },
          ]);
          setTyping(false);
          pendingContentRef.current = '';
          break;
      }
    };

    ws.connect();
    wsRef.current = ws;

    return () => {
      ws.disconnect();
    };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  // ── Load history for a session ────────────────────────────────────
  const loadHistory = useCallback(async (ws: WebSocketClient, sessionKey: string) => {
    setLoading(true);
    try {
      const res = await ws.rpc<{
        messages: ChatMessageInfo[];
        session_key: string;
        label: string | null;
      }>('chat.history', { session: sessionKey, limit: 50 });

      const mapped: ChatMessage[] = res.messages.map((m) => ({
        id: String(m.id),
        role: (m.role === 'user' ? 'user' : 'agent') as 'user' | 'agent',
        content: m.content,
        timestamp: new Date(m.timestamp * 1000),
        kind: m.kind,
      }));
      setMessages(mapped);
    } catch {
      setMessages([]);
    }
    setLoading(false);
  }, []);

  // ── Auto-scroll ───────────────────────────────────────────────────
  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages, typing]);

  // ── Session actions ───────────────────────────────────────────────
  const handleSelectSession = useCallback(
    (key: string) => {
      if (key === activeSession) return;
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
        setSessions((prev) => {
          const remaining = prev.filter((s) => s.key !== key);
          // Switch to another session if we deleted the active one
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

  // ── Send message ──────────────────────────────────────────────────
  const handleSend = () => {
    const trimmed = input.trim();
    if (!trimmed || !wsRef.current?.connected) return;

    setMessages((prev) => [
      ...prev,
      {
        id: generateUUID(),
        role: 'user',
        content: trimmed,
        timestamp: new Date(),
        kind: 'user',
      },
    ]);

    try {
      wsRef.current.sendMessage(trimmed);
      setTyping(true);
      pendingContentRef.current = '';
    } catch {
      setError('Failed to send message. Please try again.');
    }

    setInput('');
    clearDraft();
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

  return (
    <div className="flex h-[calc(100vh-3.5rem)]">
      {/* Session Sidebar */}
      <SessionSidebar
        sessions={sessions}
        activeKey={activeSession}
        collapsed={sidebarCollapsed}
        onToggle={() => setSidebarCollapsed(!sidebarCollapsed)}
        onSelect={handleSelectSession}
        onNew={handleNewSession}
        onRename={handleRenameSession}
        onDelete={handleDeleteSession}
      />

      {/* Chat area */}
      <div className="flex-1 flex flex-col min-w-0">
        {/* Connection status bar */}
        {error && (
          <div className="px-4 py-2 bg-[#ff446615] border-b border-[#ff446630] flex items-center gap-2 text-sm text-[#ff6680] animate-fade-in">
            <AlertCircle className="h-4 w-4 flex-shrink-0" />
            {error}
          </div>
        )}

        {/* Messages area */}
        <div className="flex-1 overflow-y-auto p-4 space-y-4">
          {loading ? (
            <div className="flex items-center justify-center h-full">
              <div className="h-8 w-8 border-2 border-[#0080ff30] border-t-[#0080ff] rounded-full animate-spin" />
            </div>
          ) : messages.length === 0 ? (
            <div className="flex flex-col items-center justify-center h-full text-[#334060] animate-fade-in">
              <div
                className="h-16 w-16 rounded-2xl flex items-center justify-center mb-4 animate-float"
                style={{
                  background: 'linear-gradient(135deg, #0080ff15, #0080ff08)',
                }}
              >
                <Bot className="h-8 w-8 text-[#0080ff]" />
              </div>
              <p className="text-lg font-semibold text-white mb-1">ZeroClaw Agent</p>
              <p className="text-sm text-[#556080]">Send a message to start the conversation</p>
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
                  className="flex-shrink-0 w-8 h-8 rounded-xl flex items-center justify-center"
                  style={{
                    background:
                      msg.role === 'user'
                        ? 'linear-gradient(135deg, #0080ff, #0060cc)'
                        : 'linear-gradient(135deg, #1a1a3e, #12122a)',
                  }}
                >
                  {msg.role === 'user' ? (
                    <User className="h-4 w-4 text-white" />
                  ) : (
                    <Bot className="h-4 w-4 text-[#0080ff]" />
                  )}
                </div>
                <div className="relative max-w-[75%]">
                  <div
                    className={`rounded-2xl px-4 py-3 ${
                      msg.role === 'user'
                        ? 'text-white'
                        : 'text-[#e8edf5] border border-[#1a1a3e]'
                    }`}
                    style={{
                      background:
                        msg.role === 'user'
                          ? 'linear-gradient(135deg, #0080ff, #0066cc)'
                          : 'linear-gradient(135deg, rgba(13,13,32,0.8), rgba(10,10,26,0.6))',
                    }}
                  >
                    <p className="text-sm whitespace-pre-wrap break-words">{msg.content}</p>
                    <p
                      className={`text-[10px] mt-1.5 ${
                        msg.role === 'user' ? 'text-white/50' : 'text-[#334060]'
                      }`}
                    >
                      {msg.timestamp.toLocaleTimeString()}
                    </p>
                  </div>
                  <button
                    onClick={() => handleCopy(msg.id, msg.content)}
                    aria-label="Copy message"
                    className="absolute top-1 right-1 opacity-0 group-hover:opacity-100 transition-all duration-300 p-1.5 rounded-lg bg-[#0a0a18] border border-[#1a1a3e] text-[#556080] hover:text-white hover:border-[#0080ff40]"
                  >
                    {copiedId === msg.id ? (
                      <Check className="h-3 w-3 text-[#00e68a]" />
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
                className="flex-shrink-0 w-8 h-8 rounded-xl flex items-center justify-center"
                style={{ background: 'linear-gradient(135deg, #1a1a3e, #12122a)' }}
              >
                <Bot className="h-4 w-4 text-[#0080ff]" />
              </div>
              <div
                className="rounded-2xl px-4 py-3 border border-[#1a1a3e]"
                style={{
                  background:
                    'linear-gradient(135deg, rgba(13,13,32,0.8), rgba(10,10,26,0.6))',
                }}
              >
                <div className="flex items-center gap-1.5">
                  <span
                    className="w-1.5 h-1.5 bg-[#0080ff] rounded-full animate-bounce"
                    style={{ animationDelay: '0ms' }}
                  />
                  <span
                    className="w-1.5 h-1.5 bg-[#0080ff] rounded-full animate-bounce"
                    style={{ animationDelay: '150ms' }}
                  />
                  <span
                    className="w-1.5 h-1.5 bg-[#0080ff] rounded-full animate-bounce"
                    style={{ animationDelay: '300ms' }}
                  />
                </div>
              </div>
            </div>
          )}

          <div ref={messagesEndRef} />
        </div>

        {/* Input area */}
        <div
          className="border-t border-[#1a1a3e]/40 p-4"
          style={{
            background: 'linear-gradient(180deg, rgba(8,8,24,0.9), rgba(5,5,16,0.95))',
          }}
        >
          <div className="flex items-end gap-3 max-w-4xl mx-auto">
            <div className="flex-1">
              <textarea
                ref={inputRef}
                rows={1}
                value={input}
                onChange={handleTextareaChange}
                onKeyDown={handleKeyDown}
                placeholder={connected ? 'Type a message...' : 'Connecting...'}
                disabled={!connected}
                className="input-electric w-full px-4 py-3 text-sm resize-none overflow-y-auto disabled:opacity-40"
                style={{ minHeight: '44px', maxHeight: '200px' }}
              />
            </div>
            {typing ? (
              <button
                onClick={handleAbort}
                className="flex-shrink-0 p-3 rounded-xl bg-[#ff4466] hover:bg-[#ff2244] text-white transition-colors"
                title="Stop generation"
              >
                <Square className="h-5 w-5" />
              </button>
            ) : (
              <button
                onClick={handleSend}
                disabled={!connected || !input.trim()}
                className="btn-electric flex-shrink-0 p-3 rounded-xl"
              >
                <Send className="h-5 w-5" />
              </button>
            )}
          </div>
          <div className="flex items-center justify-center mt-2 gap-2">
            <span
              className={`inline-block h-1.5 w-1.5 rounded-full glow-dot ${
                connected ? 'text-[#00e68a] bg-[#00e68a]' : 'text-[#ff4466] bg-[#ff4466]'
              }`}
            />
            <span className="text-[10px] text-[#334060]">
              {connected ? 'Connected' : 'Disconnected'}
            </span>
          </div>
        </div>
      </div>
    </div>
  );
}
