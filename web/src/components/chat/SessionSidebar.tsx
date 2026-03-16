import { useState } from 'react';
import { Plus, MessageSquare, Pencil, Trash2, PanelLeftClose, PanelLeft, Check, X, Cpu, Clock, Sparkles } from 'lucide-react';
import type { ChatSessionInfo, StatusResponse } from '@/types/api';

interface SessionSidebarProps {
  sessions: ChatSessionInfo[];
  activeKey: string | null;
  collapsed: boolean;
  status: StatusResponse | null;
  onToggle: () => void;
  onSelect: (key: string) => void;
  onNew: () => void;
  onRename: (key: string, label: string) => void;
  onDelete: (key: string) => void;
  onSummaryModelChange: (model: string | null) => void;
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
  // "anthropic/claude-sonnet-4-6" → "claude-sonnet-4-6"
  const parts = model.split('/');
  return parts[parts.length - 1] ?? model;
}

export default function SessionSidebar({
  sessions,
  activeKey,
  collapsed,
  status,
  onToggle,
  onSelect,
  onNew,
  onRename,
  onDelete,
  onSummaryModelChange,
}: SessionSidebarProps) {
  const [editingKey, setEditingKey] = useState<string | null>(null);
  const [editValue, setEditValue] = useState('');
  const [editingSummaryModel, setEditingSummaryModel] = useState(false);
  const [summaryModelInput, setSummaryModelInput] = useState('');

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

  if (collapsed) {
    return (
      <div className="flex flex-col items-center py-3 px-1 border-r border-[#1a1a3e]/40 w-10" style={{ background: 'linear-gradient(180deg, rgba(8,8,24,0.95), rgba(5,5,16,0.98))' }}>
        <button
          onClick={onToggle}
          className="p-1.5 rounded-lg text-[#556080] hover:text-white hover:bg-[#1a1a3e]/50 transition-colors"
          title="Expand sidebar"
        >
          <PanelLeft className="h-4 w-4" />
        </button>
        <button
          onClick={onNew}
          className="mt-2 p-1.5 rounded-lg text-[#0080ff] hover:bg-[#0080ff15] transition-colors"
          title="New Chat"
        >
          <Plus className="h-4 w-4" />
        </button>
      </div>
    );
  }

  return (
    <div className="flex flex-col w-[220px] border-r border-[#1a1a3e]/40 overflow-hidden" style={{ background: 'linear-gradient(180deg, rgba(8,8,24,0.95), rgba(5,5,16,0.98))' }}>
      {/* Header */}
      <div className="flex items-center justify-between px-3 py-2.5 border-b border-[#1a1a3e]/30">
        <button
          onClick={onNew}
          className="flex items-center gap-1.5 px-2 py-1 rounded-lg text-xs font-medium text-[#0080ff] hover:bg-[#0080ff15] transition-colors"
        >
          <Plus className="h-3.5 w-3.5" />
          New Chat
        </button>
        <button
          onClick={onToggle}
          className="p-1 rounded-lg text-[#556080] hover:text-white hover:bg-[#1a1a3e]/50 transition-colors"
          title="Collapse sidebar"
        >
          <PanelLeftClose className="h-3.5 w-3.5" />
        </button>
      </div>

      {/* Agent Info Panel */}
      {status && (
        <div className="px-3 py-2 border-b border-[#1a1a3e]/30 space-y-1.5">
          <div className="flex items-center gap-1.5">
            <Cpu className="h-3 w-3 text-[#0080ff] flex-shrink-0" />
            <span className="text-[10px] text-[#8890a8] truncate" title={status.model}>
              {shortModel(status.model)}
            </span>
          </div>
          <div className="flex items-center gap-1.5">
            <Sparkles className="h-3 w-3 text-[#9966ff] flex-shrink-0" />
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
                  className="flex-1 min-w-0 bg-[#0a0a18] border border-[#0080ff40] rounded px-1 py-0 text-[10px] text-white outline-none"
                  placeholder="model name or empty"
                  autoFocus
                />
                <button
                  onClick={() => {
                    const val = summaryModelInput.trim() || null;
                    onSummaryModelChange(val);
                    setEditingSummaryModel(false);
                  }}
                  className="text-[#00e68a] hover:text-white"
                >
                  <Check className="h-2.5 w-2.5" />
                </button>
                <button
                  onClick={() => setEditingSummaryModel(false)}
                  className="text-[#ff4466] hover:text-white"
                >
                  <X className="h-2.5 w-2.5" />
                </button>
              </div>
            ) : (
              <span
                className="text-[10px] text-[#556080] truncate cursor-pointer hover:text-[#8890a8] transition-colors"
                title={`Summary: ${status.summary_model ?? 'same as primary'} (click to change)`}
                onClick={() => {
                  setSummaryModelInput(status.summary_model ?? '');
                  setEditingSummaryModel(true);
                }}
              >
                {status.summary_model ? shortModel(status.summary_model) : 'auto'}
              </span>
            )}
          </div>
          <div className="flex items-center gap-1.5">
            <Clock className="h-3 w-3 text-[#334060] flex-shrink-0" />
            <span className="text-[10px] text-[#334060]">
              {formatUptime(status.uptime_seconds)}
            </span>
          </div>
        </div>
      )}

      {/* Sessions list */}
      <div className="flex-1 overflow-y-auto py-1">
        {sessions.length === 0 && (
          <p className="text-[10px] text-[#334060] text-center mt-4 px-2">No sessions yet</p>
        )}
        {sessions.map((s) => {
          const isActive = s.key === activeKey;
          const label = s.label || `Session`;

          return (
            <div
              key={s.key}
              onClick={() => onSelect(s.key)}
              className={`group relative px-3 py-2 mx-1 rounded-lg cursor-pointer transition-colors ${
                isActive
                  ? 'bg-[#0080ff15] border border-[#0080ff30]'
                  : 'hover:bg-[#1a1a3e]/30 border border-transparent'
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
                    className="flex-1 bg-[#0a0a18] border border-[#0080ff40] rounded px-1.5 py-0.5 text-[11px] text-white outline-none"
                    autoFocus
                    onClick={(e) => e.stopPropagation()}
                  />
                  <button onClick={(e) => { e.stopPropagation(); confirmRename(); }} className="text-[#00e68a] hover:text-white">
                    <Check className="h-3 w-3" />
                  </button>
                  <button onClick={(e) => { e.stopPropagation(); cancelRename(); }} className="text-[#ff4466] hover:text-white">
                    <X className="h-3 w-3" />
                  </button>
                </div>
              ) : (
                <>
                  <div className="flex items-center gap-1.5">
                    <MessageSquare className={`h-3 w-3 flex-shrink-0 ${isActive ? 'text-[#0080ff]' : 'text-[#334060]'}`} />
                    <span className={`text-[11px] font-medium truncate ${isActive ? 'text-white' : 'text-[#8890a8]'}`}>
                      {label}
                    </span>
                  </div>
                  {s.current_goal ? (
                    <p className="text-[10px] text-[#556080] truncate mt-0.5 pl-[18px]">{s.current_goal}</p>
                  ) : s.preview ? (
                    <p className="text-[10px] text-[#334060] truncate mt-0.5 pl-[18px]">{s.preview}</p>
                  ) : null}
                  <p className="text-[9px] text-[#223050] mt-0.5 pl-[18px]">
                    {timeAgo(s.last_active)}
                    {(s.input_tokens > 0 || s.output_tokens > 0) && (
                      <span className="ml-1">{((s.input_tokens + s.output_tokens) / 1000).toFixed(1)}k tok</span>
                    )}
                  </p>

                  {/* Hover actions */}
                  <div className="absolute right-1 top-1.5 opacity-0 group-hover:opacity-100 transition-opacity flex gap-0.5">
                    <button
                      onClick={(e) => { e.stopPropagation(); startRename(s.key, label); }}
                      className="p-1 rounded text-[#556080] hover:text-white hover:bg-[#1a1a3e]"
                      title="Rename"
                    >
                      <Pencil className="h-2.5 w-2.5" />
                    </button>
                    <button
                      onClick={(e) => { e.stopPropagation(); onDelete(s.key); }}
                      className="p-1 rounded text-[#556080] hover:text-[#ff4466] hover:bg-[#1a1a3e]"
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
