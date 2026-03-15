interface LaneDotProps {
  lane: string;
}

export default function LaneDot({ lane }: LaneDotProps) {
  if (lane === 'normal') return null;

  const color = lane === 'quarantine' ? 'bg-orange-400' : 'bg-red-400';

  return (
    <span
      className={`inline-block h-2 w-2 rounded-full ${color}`}
      title={lane}
    />
  );
}
