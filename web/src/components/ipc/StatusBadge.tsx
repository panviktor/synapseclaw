interface StatusBadgeProps {
  status: string;
}

interface StyleDef {
  bg: string;
  text: string;
  dot?: string;
}

const statusStyles: Record<string, StyleDef> = {
  online: { bg: 'bg-emerald-500/20', text: 'text-emerald-400', dot: 'bg-emerald-400' },
  stale: { bg: 'bg-yellow-500/20', text: 'text-yellow-400', dot: 'bg-yellow-400' },
  disabled: { bg: 'bg-gray-500/20', text: 'text-gray-400' },
  revoked: { bg: 'bg-red-500/20', text: 'text-red-400' },
  quarantined: { bg: 'bg-orange-500/20', text: 'text-orange-400' },
  ephemeral: { bg: 'bg-purple-500/20', text: 'text-purple-400' },
  interrupted: { bg: 'bg-gray-500/20', text: 'text-gray-500' },
};

const defaultStyle: StyleDef = { bg: 'bg-gray-500/20', text: 'text-gray-400' };

export default function StatusBadge({ status }: StatusBadgeProps) {
  const style = statusStyles[status] ?? defaultStyle;

  return (
    <span className={`inline-flex items-center gap-1.5 px-2 py-0.5 rounded-full text-xs font-medium ${style.bg} ${style.text}`}>
      {style.dot && (
        <span className={`h-1.5 w-1.5 rounded-full ${style.dot} ${status === 'online' ? 'animate-pulse' : ''}`} />
      )}
      {status}
    </span>
  );
}
