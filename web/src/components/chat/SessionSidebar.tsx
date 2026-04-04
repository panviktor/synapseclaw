import { useState } from 'react';
import { Plus, MessageSquare, Pencil, Trash2, PanelLeftClose, PanelLeft, Check, X, Cpu, Clock, Sparkles, Users, Search } from 'lucide-react';
import type { ChatSessionInfo, StatusResponse } from '@/types/api';
import type { AgentEntry } from '@/lib/api';
import { t } from '@/lib/i18n';

interface SessionSidebarProps {
  sessions: ChatSessionInfo[];
  activeKey: string | null;
  collapsed: boolean;
  status: StatusResponse | null;
  agents: AgentEntry[];
  activeAgent: string | null;
  onToggle: () => void;
  onSelect: (key: string) => void;
  onNew: () => void;
  onRename: (key: string, label: string) => void;
  onDelete: (key: string) => void;
  onSummaryModelChange: (model: string | null) => void;
  onAgentChange: (agentId: string | null) => void;
}

function timeAgo(epochSecs: number): string {
  const diff = Math.floor(Date.now() / 1000) - epochSecs;
  if (diff < 60) return 'just now';
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
  return `${Math.floor(diff / 86400)}d ago`;
}

function formatUptime(secs: number): string {
  if (secs < 3600) return `${Math.floor(secs / 60)}m`;
  if (secs < 86400) return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`;
  return `${Math.floor(secs / 86400)}d ${Math.floor((secs % 86400) / 3600)}h`;
}

function shortModel(model: string): string {
  const parts = model.split('/');
  return parts[parts.length - 1] ?? model;
}

const CHANNEL_BADGES: Record<string, { bg: string; text: string; label: string }> = {
  matrix: { bg: 'bg-purple-500/15', text: 'text-purple-400', label: 'matrix' },
  telegram: { bg: 'bg-blue-500/15', text: 'text-blue-400', label: 'tg' },
  discord: { bg: 'bg-indigo-500/15', text: 'text-indigo-400', label: 'discord' },
  slack: { bg: 'bg-green-500/15', text: 'text-green-400', label: 'slack' },
  irc: { bg: 'bg-gray-500/15', text: 'text-gray-400', label: 'irc' },
  signal: { bg: 'bg-sky-500/15', text: 'text-sky-400', label: 'signal' },
};

function ChannelBadge({ channel }: { channel: string }) {
  const badge = CHANNEL_BADGES[channel] ?? { bg: 'bg-gray-500/15', text: 'text-gray-400', label: channel };
  return (
    <span className={`text-[8px] px-1 py-0.5 rounded font-semibold uppercase tracking-wide flex-shrink-0 ${badge.bg} ${badge.text}`}>
      {badge.label}
    </span>
  );
}

export default function SessionSidebar({
  sessions,
  activeKey,
  collapsed,
  status,
  agents,
  activeAgent,
  onToggle,
  onSelect,
  onNew,
  onRename,
  onDelete,
  onSummaryModelChange,
  onAgentChange,
}: SessionSidebarProps) {
  const [editingKey, setEditingKey] = useState<string | null>(null);
  const [editValue, setEditValue] = useState('');
  const [editingSummaryModel, setEditingSummaryModel] = useState(false);
  const [summaryModelInput, setSummaryModelInput] = useState('');
  const [searchQuery, setSearchQuery] = useState('');

  const startRename = (key: string, currentLabel: string) => {
    setEditingKey(key);
    setEditValue(currentLabel);
  };

  const confirmRename = () => {
    if (editingKey && editValue.trim()) {
      onRename(editingKey, editValue.trim());
    }
    setEditingKey(null);
  };

  const cancelRename = () => setEditingKey(null);

  // Filter sessions by search query
  const filteredSessions = searchQuery.trim()
    ? sessions.filter((s) => {
        const q = searchQuery.toLowerCase();
        return (
          (s.label ?? '').toLowerCase().includes(q) ||
          (s.preview ?? '').toLowerCase().includes(q) ||
          (s.channel ?? '').toLowerCase().includes(q) ||
          s.key.toLowerCase().includes(q)
        );
      })
    : sessions;

  if (collapsed) {
    return (
      <div className="flex flex-col items-center py-3 px-1 border-r border-[var(--border-default)] w-10 bg-[var(--bg-secondary)]">
        <button
          onClick={onToggle}
          className="p-1.5 rounded-lg text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[#E5E3E0] transition-colors"
          title="Expand sidebar"
        >
          <PanelLeft className="h-4 w-4" />
        </button>
        <button
          onClick={onNew}
          className="mt-2 p-1.5 rounded-lg text-[var(--accent-primary)] hover:bg-[#D95A1E]/10 transition-colors"
          title="New Chat"
        >
          <Plus className="h-4 w-4" />
        </button>
      </div>
    );
  }

  return (
    <div className="flex flex-col w-[220px] border-r border-[var(--border-default)] overflow-hidden bg-[var(--bg-secondary)]">
      {/* Header */}
      <div className="flex items-center justify-between px-3 py-2.5 border-b border-[var(--border-default)]">
        <button
          onClick={onNew}
          className="flex items-center gap-1.5 px-2 py-1 rounded-lg text-xs font-medium text-[var(--accent-primary)] hover:bg-[#D95A1E]/10 transition-colors"
        >
          <Plus className="h-3.5 w-3.5" />
          New Chat
        </button>
        <button
          onClick={onToggle}
          className="p-1 rounded-lg text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[#E5E3E0] transition-colors"
          title="Collapse sidebar"
        >
          <PanelLeftClose className="h-3.5 w-3.5" />
        </button>
      </div>

      {/* Search */}
      <div className="px-3 py-2 border-b border-[var(--border-default)]">
        <div className="relative">
          <Search className="absolute left-2 top-1/2 -translate-y-1/2 h-3 w-3 text-[var(--text-placeholder)]" />
          <input
            type="text"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            placeholder="Search..."
            className="w-full bg-[var(--bg-card)] border border-[var(--border-default)] rounded-lg pl-7 pr-2 py-1 text-[10px] text-[var(--text-primary)] placeholder:text-[var(--text-placeholder)] outline-none focus:border-[var(--accent-primary)]/40 transition-colors"
          />
        </div>
      </div>

      {/* Agent Selector (Phase 3.8) */}
      {agents.length > 0 && (
        <div className="px-3 py-2 border-b border-[var(--border-default)]">
          <div className="flex items-center gap-1.5 mb-1">
            <Users className="h-3 w-3 text-[var(--text-muted)] flex-shrink-0" />
            <span className="text-[10px] text-[var(--text-muted)] uppercase tracking-wide">Agent</span>
          </div>
          <select
            value={activeAgent ?? ''}
            onChange={(e) => onAgentChange(e.target.value || null)}
            className="w-full bg-[var(--bg-card)] border border-[var(--border-default)] rounded px-2 py-1 text-[11px] text-[var(--text-primary)] outline-none focus:border-[var(--accent-primary)]/40 transition-colors"
          >
            <option value="">Local (this instance)</option>
            {agents.map((a) => (
              <option key={a.agent_id} value={a.agent_id}>
                {a.agent_id} {a.role ? `(${a.role})` : ''} {a.status === 'offline' ? ' [offline]' : ''}
              </option>
            ))}
          </select>
        </div>
      )}

      {/* Agent Info Panel */}
      {status && (
        <div className="px-3 py-2 border-b border-[var(--border-default)] space-y-1.5">
          <div className="flex items-center gap-1.5">
            <Cpu className="h-3 w-3 text-[var(--accent-primary)] flex-shrink-0" />
            <span className="text-[10px] text-[var(--text-secondary)] truncate" title={status.model}>
              {shortModel(status.model)}
            </span>
          </div>
          <div className="flex items-center gap-1.5">
            <Sparkles className="h-3 w-3 text-[#C9872C] flex-shrink-0" />
            {editingSummaryModel ? (
              <div className="flex items-center gap-1 flex-1 min-w-0">
                <input
                  type="text"
                  value={summaryModelInput}
                  onChange={(e) => setSummaryModelInput(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') {
                      const val = summaryModelInput.trim() || null;
                      onSummaryModelChange(val);
                      setEditingSummaryModel(false);
                    }
                    if (e.key === 'Escape') setEditingSummaryModel(false);
                  }}
                  className="flex-1 min-w-0 bg-[var(--bg-card)] border border-[var(--accent-primary)]/40 rounded px-1 py-0 text-[10px] text-[var(--text-primary)] outline-none"
                  placeholder="model name or empty"
                  autoFocus
                />
                <button
                  onClick={() => {
                    const val = summaryModelInput.trim() || null;
                    onSummaryModelChange(val);
                    setEditingSummaryModel(false);
                  }}
                  className="text-[#2D8A4E] hover:text-[var(--text-primary)]"
                >
                  <Check className="h-2.5 w-2.5" />
                </button>
                <button
                  onClick={() => setEditingSummaryModel(false)}
                  className="text-[#C73E3E] hover:text-[var(--text-primary)]"
                >
                  <X className="h-2.5 w-2.5" />
                </button>
              </div>
            ) : (
              <span
                className="text-[10px] text-[var(--text-muted)] truncate cursor-pointer hover:text-[var(--text-secondary)] transition-colors"
                title={`${t('agent.summary_model')}: ${status.summary_model ?? 'same as primary'} (click to change)`}
                onClick={() => {
                  setSummaryModelInput(status.summary_model ?? '');
                  setEditingSummaryModel(true);
                }}
              >
                <span className="text-[var(--text-placeholder)]">{t('agent.summary_model')}:</span>{' '}
                {status.summary_model ? shortModel(status.summary_model) : t('agent.summary_auto')}
              </span>
            )}
          </div>
          {status.embedding_model && (
            <div className="flex items-center gap-1.5">
              <Search className="h-3 w-3 text-[#7C6BAF] flex-shrink-0" />
              <span className="text-[10px] text-[var(--text-muted)] truncate" title={`${status.embedding_provider ?? ''} / ${status.embedding_model}`}>
                {shortModel(status.embedding_model)}
              </span>
            </div>
          )}
          <div className="flex items-center gap-1.5">
            <Clock className="h-3 w-3 text-[var(--text-placeholder)] flex-shrink-0" />
            <span className="text-[10px] text-[var(--text-placeholder)]">
              {formatUptime(status.uptime_seconds)}
            </span>
          </div>
        </div>
      )}

      {/* Unified sessions list */}
      <div className="flex-1 overflow-y-auto py-1">
        {filteredSessions.length === 0 && !searchQuery && (
          <p className="text-[10px] text-[var(--text-placeholder)] text-center mt-4 px-2">
            No sessions yet. Start a new chat or conversations from channels will appear here.
          </p>
        )}
        {filteredSessions.length === 0 && searchQuery && (
          <p className="text-[10px] text-[var(--text-placeholder)] text-center mt-4 px-2">
            No sessions matching "{searchQuery}"
          </p>
        )}
        {filteredSessions.map((s) => {
          const isActive = s.key === activeKey;
          const isChannel = s.kind === 'channel';
          const label = s.label || (isChannel ? s.key : 'Session');

          return (
            <div
              key={s.key}
              onClick={() => onSelect(s.key)}
              className={`group relative px-3 py-2 mx-1 rounded-lg cursor-pointer transition-colors ${
                isActive
                  ? 'bg-[#D95A1E]/10 border border-[var(--accent-primary)]/20'
                  : 'hover:bg-[#E5E3E0]/50 border border-transparent'
              }`}
            >
              {editingKey === s.key ? (
                <div className="flex items-center gap-1">
                  <input
                    type="text"
                    value={editValue}
                    onChange={(e) => setEditValue(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter') confirmRename();
                      if (e.key === 'Escape') cancelRename();
                    }}
                    className="flex-1 bg-[var(--bg-card)] border border-[var(--accent-primary)]/40 rounded px-1.5 py-0.5 text-[11px] text-[var(--text-primary)] outline-none"
                    autoFocus
                    onClick={(e) => e.stopPropagation()}
                  />
                  <button onClick={(e) => { e.stopPropagation(); confirmRename(); }} className="text-[#2D8A4E] hover:text-[var(--text-primary)]">
                    <Check className="h-3 w-3" />
                  </button>
                  <button onClick={(e) => { e.stopPropagation(); cancelRename(); }} className="text-[#C73E3E] hover:text-[var(--text-primary)]">
                    <X className="h-3 w-3" />
                  </button>
                </div>
              ) : (
                <>
                  <div className="flex items-center gap-1.5">
                    {isChannel && s.channel ? (
                      <ChannelBadge channel={s.channel} />
                    ) : (
                      <MessageSquare className={`h-3 w-3 flex-shrink-0 ${isActive ? 'text-[var(--accent-primary)]' : 'text-[var(--text-placeholder)]'}`} />
                    )}
                    <span className={`text-[11px] font-medium truncate ${isActive ? 'text-[var(--text-primary)]' : 'text-[var(--text-secondary)]'}`}>
                      {label}
                    </span>
                  </div>
                  {s.session_summary ? (
                    <p className="text-[10px] text-[var(--text-muted)] truncate mt-0.5 pl-[18px]" title={s.session_summary}>{s.session_summary}</p>
                  ) : s.current_goal ? (
                    <p className="text-[10px] text-[var(--text-muted)] truncate mt-0.5 pl-[18px]">{s.current_goal}</p>
                  ) : s.preview ? (
                    <p className="text-[10px] text-[var(--text-placeholder)] truncate mt-0.5 pl-[18px]">{s.preview}</p>
                  ) : null}
                  <p className="text-[9px] text-[var(--text-placeholder)] mt-0.5 pl-[18px]">
                    {timeAgo(s.last_active)}
                    {isChannel ? (
                      s.message_count > 0 && <span className="ml-1">{s.message_count} msgs</span>
                    ) : (
                      (s.input_tokens > 0 || s.output_tokens > 0) && (
                        <span className="ml-1">{((s.input_tokens + s.output_tokens) / 1000).toFixed(1)}k tok</span>
                      )
                    )}
                  </p>

                  {/* Hover actions */}
                  <div className="absolute right-1 top-1.5 opacity-0 group-hover:opacity-100 transition-opacity flex gap-0.5">
                    {!isChannel && (
                      <button
                        onClick={(e) => { e.stopPropagation(); startRename(s.key, label); }}
                        className="p-1 rounded text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[#E5E3E0]"
                        title="Rename"
                      >
                        <Pencil className="h-2.5 w-2.5" />
                      </button>
                    )}
                    <button
                      onClick={(e) => { e.stopPropagation(); onDelete(s.key); }}
                      className="p-1 rounded text-[var(--text-muted)] hover:text-[#C73E3E] hover:bg-[#E5E3E0]"
                      title="Delete"
                    >
                      <Trash2 className="h-2.5 w-2.5" />
                    </button>
                  </div>
                </>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
