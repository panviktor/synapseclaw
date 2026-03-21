import { Link } from 'react-router-dom';
import TrustBadge from './TrustBadge';

interface AgentLinkProps {
  agentId: string;
  trustLevel?: number | null;
  showTrust?: boolean;
}

export default function AgentLink({ agentId, trustLevel, showTrust = true }: AgentLinkProps) {
  return (
    <Link
      to={`/ipc/fleet/${encodeURIComponent(agentId)}`}
      className="inline-flex items-center gap-1.5 hover:text-[var(--accent-primary)] transition-colors"
    >
      <span className="font-mono text-sm">{agentId}</span>
      {showTrust && trustLevel !== undefined && (
        <TrustBadge level={trustLevel} />
      )}
    </Link>
  );
}
