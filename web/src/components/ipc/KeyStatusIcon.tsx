import { ShieldCheck, ShieldOff } from 'lucide-react';

interface KeyStatusIconProps {
  publicKey: string | null;
}

export default function KeyStatusIcon({ publicKey }: KeyStatusIconProps) {
  if (publicKey) {
    return (
      <span title="Ed25519 key registered" className="text-emerald-400">
        <ShieldCheck className="h-4 w-4" />
      </span>
    );
  }

  return (
    <span title="No key registered (unsigned)" className="text-gray-500">
      <ShieldOff className="h-4 w-4" />
    </span>
  );
}
