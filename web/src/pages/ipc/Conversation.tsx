import { useState, useEffect } from 'react';
import { useSearchParams } from 'react-router-dom';
import { apiFetch } from '@/lib/api';
import AgentLink from '@/components/ipc/AgentLink';
import { TimeAbsolute } from '@/components/ipc/TimeAgo';

interface ChatMessage {
  id: number;
  session_key: string;
  kind: string;
  role: string | null;
  content: string;
  tool_name: string | null;
  run_id: string | null;
  timestamp: number;
}

export default function Conversation() {
  const [searchParams] = useSearchParams();
  const agentId = searchParams.get('agent') ?? '';
  const sessionKey = searchParams.get('key') ?? '';
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!agentId || !sessionKey) return;
    setLoading(true);
    setError(null);

    const url = `/api/agents/${encodeURIComponent(agentId)}/chat/sessions/${encodeURIComponent(sessionKey)}/messages?limit=200`;
    apiFetch<{ messages: ChatMessage[] }>(url)
      .then((data) => setMessages(data.messages))
      .catch((err) => setError(err instanceof Error ? err.message : 'Failed to load'))
      .finally(() => setLoading(false));
  }, [agentId, sessionKey]);

  if (!agentId || !sessionKey) {
    return (
      <div className="p-6 text-[var(--text-secondary)]">
        Missing agent or session key. Navigate here from the Activity feed.
      </div>
    );
  }

  return (
    <div className="p-6 space-y-4 animate-fade-in">
      <div className="flex items-center gap-3">
        <h1 className="text-xl font-bold text-gradient">Conversation</h1>
        <AgentLink agentId={agentId} />
        <span className="text-xs text-[var(--text-secondary)] font-mono bg-[var(--bg-primary)] px-2 py-1 rounded-lg">
          {sessionKey}
        </span>
      </div>

      {loading && (
        <div className="text-center py-12 text-[var(--text-secondary)]">Loading messages...</div>
      )}

      {error && (
        <div className="bg-red-500/10 border border-red-500/30 rounded-xl p-3 text-sm text-red-400">
          {error}
        </div>
      )}

      {!loading && !error && messages.length === 0 && (
        <div className="text-center py-12 text-[var(--text-secondary)]">No messages found</div>
      )}

      {messages.length > 0 && (
        <div className="glass-card overflow-hidden">
          <div className="divide-y divide-[var(--bg-secondary)] max-h-[70vh] overflow-y-auto">
            {messages.map((msg) => (
              <div
                key={msg.id}
                className={`px-4 py-3 ${msg.role === 'user' ? 'bg-[var(--glow-secondary)]' : ''}`}
              >
                <div className="flex items-center gap-2 mb-1">
                  <span className={`text-xs font-semibold ${
                    msg.role === 'user' ? 'text-blue-400' :
                    msg.role === 'assistant' ? 'text-green-400' :
                    'text-[var(--text-secondary)]'
                  }`}>
                    {msg.role ?? msg.kind}
                  </span>
                  {msg.tool_name && (
                    <span className="text-[10px] text-purple-400 bg-purple-500/10 px-1.5 py-0.5 rounded">
                      {msg.tool_name}
                    </span>
                  )}
                  <span className="text-[10px] text-[var(--text-secondary)] ml-auto">
                    <TimeAbsolute timestamp={msg.timestamp} />
                  </span>
                </div>
                <div className="text-sm text-[var(--text-muted)] whitespace-pre-wrap break-words">
                  {msg.content}
                </div>
              </div>
            ))}
          </div>
        </div>
      )}

      <div className="text-xs text-[var(--text-secondary)] text-center">
        {messages.length > 0 && `${messages.length} message${messages.length !== 1 ? 's' : ''}`}
        {' — read-only view'}
      </div>
    </div>
  );
}
