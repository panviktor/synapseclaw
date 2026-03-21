interface TimeAgoProps {
  timestamp: number; // unix seconds
  staleThreshold?: number; // seconds before marking as stale (red)
}

function formatTimeAgo(unixSeconds: number): string {
  const now = Math.floor(Date.now() / 1000);
  const diff = now - unixSeconds;

  if (diff < 0) return 'in the future';
  if (diff < 60) return `${diff}s ago`;
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
  return `${Math.floor(diff / 86400)}d ago`;
}

function formatAbsolute(unixSeconds: number): string {
  return new Date(unixSeconds * 1000).toISOString().replace('T', ' ').slice(0, 19);
}

export default function TimeAgo({ timestamp, staleThreshold }: TimeAgoProps) {
  const now = Math.floor(Date.now() / 1000);
  const isStale = staleThreshold !== undefined && (now - timestamp) > staleThreshold;

  return (
    <span
      className={`text-xs ${isStale ? 'text-red-400' : 'text-[var(--text-muted)]'}`}
      title={formatAbsolute(timestamp)}
    >
      {formatTimeAgo(timestamp)}
    </span>
  );
}

export function TimeAbsolute({ timestamp }: { timestamp: number }) {
  return (
    <span className="text-xs text-[var(--text-muted)]" title={formatTimeAgo(timestamp)}>
      {formatAbsolute(timestamp)}
    </span>
  );
}

export function TimeUntil({ timestamp }: { timestamp: number }) {
  const now = Math.floor(Date.now() / 1000);
  const diff = timestamp - now;

  if (diff <= 0) {
    return <span className="text-xs text-orange-400">expired {formatTimeAgo(timestamp)}</span>;
  }

  let label: string;
  if (diff < 60) label = `in ${diff}s`;
  else if (diff < 3600) label = `in ${Math.floor(diff / 60)}m`;
  else label = `in ${Math.floor(diff / 3600)}h`;

  return (
    <span className="text-xs text-[var(--text-muted)]" title={formatAbsolute(timestamp)}>
      {label}
    </span>
  );
}
