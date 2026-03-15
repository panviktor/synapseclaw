import { useState } from 'react';
import { Link } from 'react-router-dom';
import type { IpcMessage } from '@/types/ipc';
import KindBadge from './KindBadge';
import LaneDot from './LaneDot';
import AgentLink from './AgentLink';
import { TimeAbsolute } from './TimeAgo';

interface MessageDetailProps {
  message: IpcMessage;
}

export default function MessageDetail({ message }: MessageDetailProps) {
  const [showRaw, setShowRaw] = useState(false);

  const payloadPreview = message.payload.length > 200
    ? message.payload.slice(0, 200) + '...'
    : message.payload;

  return (
    <div className="p-4 rounded-lg bg-[#0a0a18] border border-[#1a1a3e]/50 space-y-3">
      {/* Header row */}
      <div className="flex items-center gap-3 flex-wrap">
        <AgentLink agentId={message.from_agent} trustLevel={message.from_trust_level} />
        <span className="text-[#556080]">&rarr;</span>
        <AgentLink agentId={message.to_agent} showTrust={false} />
        <KindBadge kind={message.kind} />
        <LaneDot lane={message.lane} />
      </div>

      {/* Metadata */}
      <div className="flex items-center gap-4 text-xs text-[#556080]">
        <span>ID: {message.id}</span>
        <span>seq: {message.seq}</span>
        {message.session_id && (
          <Link to={`/ipc/sessions?session_id=${encodeURIComponent(message.session_id)}`} className="text-[#0080ff] hover:underline">
            session: {message.session_id}
          </Link>
        )}
        <span>priority: {message.priority}</span>
        <TimeAbsolute timestamp={message.created_at} />
      </div>

      {/* Flags */}
      <div className="flex items-center gap-2">
        {message.promoted && (
          <span className="text-xs px-1.5 py-0.5 rounded bg-yellow-500/20 text-yellow-400">promoted</span>
        )}
        {message.blocked && (
          <span className="text-xs px-1.5 py-0.5 rounded bg-red-500/20 text-red-400">
            blocked{message.blocked_reason ? `: ${message.blocked_reason}` : ''}
          </span>
        )}
        {message.read && (
          <span className="text-xs px-1.5 py-0.5 rounded bg-gray-500/20 text-gray-400">read</span>
        )}
      </div>

      {/* Payload */}
      <div className="space-y-1">
        <div className="flex items-center justify-between">
          <span className="text-xs text-[#556080] uppercase tracking-wider">Payload</span>
          <button
            onClick={() => setShowRaw(!showRaw)}
            className="text-xs text-[#0080ff] hover:text-[#0080ff]/80 transition-colors"
          >
            {showRaw ? 'Redacted' : 'Raw'}
          </button>
        </div>
        <pre className="text-sm text-[#8892a8] whitespace-pre-wrap break-all bg-[#050510] rounded p-3 max-h-64 overflow-auto">
          {showRaw ? message.payload : payloadPreview}
        </pre>
      </div>
    </div>
  );
}
