interface KindBadgeProps {
  kind: string;
}

const kindStyles: Record<string, { bg: string; text: string }> = {
  task: { bg: 'bg-blue-500/20', text: 'text-blue-400' },
  query: { bg: 'bg-purple-500/20', text: 'text-purple-400' },
  result: { bg: 'bg-emerald-500/20', text: 'text-emerald-400' },
  text: { bg: 'bg-gray-500/20', text: 'text-gray-400' },
  escalation: { bg: 'bg-orange-500/20', text: 'text-orange-400' },
  promoted_quarantine: { bg: 'bg-yellow-500/20', text: 'text-yellow-400' },
};

const defaultStyle = { bg: 'bg-gray-500/20', text: 'text-gray-400' };

export default function KindBadge({ kind }: KindBadgeProps) {
  const style = kindStyles[kind] ?? defaultStyle;

  return (
    <span className={`inline-flex items-center px-1.5 py-0.5 rounded text-xs font-medium ${style.bg} ${style.text}`}>
      {kind}
    </span>
  );
}
