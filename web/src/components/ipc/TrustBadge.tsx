interface TrustBadgeProps {
  level: number | null;
}

const tiers = [
  { key: 'coordinator', bg: 'bg-emerald-500/20', text: 'text-emerald-400' },
  { key: 'privileged', bg: 'bg-blue-500/20', text: 'text-blue-400' },
  { key: 'worker', bg: 'bg-yellow-500/20', text: 'text-yellow-400' },
  { key: 'restricted', bg: 'bg-red-500/20', text: 'text-red-400' },
] as const;

function getTier(level: number | null) {
  if (level === null || level <= 1) return tiers[0];
  if (level === 2) return tiers[1];
  if (level === 3) return tiers[2];
  return tiers[3];
}

export default function TrustBadge({ level }: TrustBadgeProps) {
  const tier = getTier(level);
  const displayLevel = level ?? 0;

  return (
    <span className={`inline-flex items-center px-1.5 py-0.5 rounded text-xs font-mono font-medium ${tier.bg} ${tier.text}`}>
      L{displayLevel}
    </span>
  );
}
