import { Cpu, Radio, Shield, Sparkles } from 'lucide-react';
import type { AgentEntry } from '@/lib/api';

interface AgentRailProps {
  agents: AgentEntry[];
  activeAgent: string | null;
  connected: boolean;
  typing: boolean;
  sessionCount: number;
  onSelect: (agentId: string | null) => void;
}

function monogram(label: string): string {
  return label
    .split(/[\s_-]+/)
    .map((part) => part[0] ?? '')
    .join('')
    .slice(0, 2)
    .toUpperCase();
}

function heatBars(level: number) {
  return Array.from({ length: 5 }, (_, idx) => idx < level);
}

function channelLabel(channels: string[]): string | null {
  if (channels.length === 0) return null;
  if (channels.length === 1) return channels[0] ?? null;
  return `${channels[0]} +${channels.length - 1}`;
}

export default function AgentRail({
  agents,
  activeAgent,
  connected,
  typing,
  sessionCount,
  onSelect,
}: AgentRailProps) {
  const localSelected = activeAgent === null;

  return (
    <div className="flex items-center gap-3 overflow-x-auto pb-1">
      <button
        onClick={() => onSelect(null)}
        className={`group relative min-w-[210px] rounded-2xl border px-4 py-3 text-left transition-all duration-300 ${
          localSelected
            ? 'border-[var(--accent-primary)] bg-[linear-gradient(135deg,var(--glow-primary),transparent_65%)] shadow-[0_10px_30px_var(--glow-primary)] -translate-y-0.5'
            : 'border-[var(--border-default)] bg-[var(--bg-card)] hover:border-[var(--accent-primary)]/35 hover:-translate-y-0.5'
        }`}
      >
        <div className="absolute inset-x-4 top-0 h-px bg-gradient-to-r from-transparent via-[var(--accent-primary)]/60 to-transparent" />
        <div className="flex items-start justify-between gap-3">
          <div className="flex items-center gap-3 min-w-0">
            <div className={`relative flex h-11 w-11 items-center justify-center rounded-2xl border ${
              localSelected ? 'border-[var(--accent-primary)]/35 bg-[var(--bg-card)]' : 'border-[var(--border-default)] bg-[var(--bg-secondary)]'
            }`}>
              <Cpu className="h-5 w-5 text-[var(--accent-primary)]" />
              <span
                className={`absolute -right-1 -top-1 h-3.5 w-3.5 rounded-full border-2 border-[var(--bg-card)] ${
                  connected ? 'bg-[var(--status-success)]' : 'bg-[var(--text-placeholder)]'
                } ${localSelected && typing ? 'animate-pulse-glow' : ''}`}
              />
            </div>
            <div className="min-w-0">
              <p className="text-[10px] font-semibold uppercase tracking-[0.24em] text-[var(--text-placeholder)]">
                Agent
              </p>
              <p className="truncate text-sm font-semibold text-[var(--text-primary)]">
                Local Runtime
              </p>
              <p className="truncate text-xs text-[var(--text-muted)]">
                {sessionCount} sessions {typing ? '· active run' : '· ready'}
              </p>
            </div>
          </div>
          <span className="rounded-full border border-[var(--border-default)] bg-[var(--bg-secondary)] px-2 py-1 text-[10px] font-semibold uppercase tracking-wide text-[var(--text-muted)]">
            local
          </span>
        </div>
        <div className="mt-3 flex items-end gap-1">
          {heatBars(Math.min(5, Math.max(1, Math.ceil(sessionCount / 2)))).map((active, idx) => (
            <span
              key={idx}
              className="rounded-full transition-all duration-300"
              style={{
                width: '9px',
                height: `${14 + idx * 4}px`,
                background: active
                  ? 'linear-gradient(180deg, rgba(217,90,30,0.95), rgba(217,90,30,0.35))'
                  : 'var(--border-default)',
                opacity: active ? 1 : 0.5,
              }}
            />
          ))}
        </div>
      </button>

      {agents.map((agent) => {
        const selected = activeAgent === agent.agent_id;
        const bars = heatBars(Math.min(5, Math.max(1, agent.channels.length + (agent.status === 'online' ? 1 : 0))));
        const label = channelLabel(agent.channels);
        return (
          <button
            key={agent.agent_id}
            onClick={() => onSelect(agent.agent_id)}
            className={`group relative min-w-[210px] rounded-2xl border px-4 py-3 text-left transition-all duration-300 ${
              selected
                ? 'border-[var(--accent-primary)] bg-[linear-gradient(145deg,var(--glow-primary),transparent_60%)] shadow-[0_10px_30px_var(--glow-primary)] -translate-y-0.5'
                : 'border-[var(--border-default)] bg-[var(--bg-card)] hover:border-[var(--accent-primary)]/35 hover:-translate-y-0.5'
            }`}
          >
            <div className="absolute inset-x-4 top-0 h-px bg-gradient-to-r from-transparent via-[var(--accent-primary)]/60 to-transparent" />
            <div className="flex items-start justify-between gap-3">
              <div className="flex items-center gap-3 min-w-0">
                <div className={`relative flex h-11 w-11 items-center justify-center rounded-2xl border ${
                  selected ? 'border-[var(--accent-primary)]/35 bg-[var(--bg-card)]' : 'border-[var(--border-default)] bg-[var(--bg-secondary)]'
                }`}>
                  <span className="text-sm font-bold tracking-wide text-[var(--text-primary)]">
                    {monogram(agent.agent_id)}
                  </span>
                  <span
                    className={`absolute -right-1 -top-1 h-3.5 w-3.5 rounded-full border-2 border-[var(--bg-card)] ${
                      agent.status === 'online'
                        ? 'bg-[var(--status-success)]'
                        : agent.status === 'error'
                          ? 'bg-[var(--status-error)]'
                          : 'bg-[var(--text-placeholder)]'
                    } ${selected && typing ? 'animate-pulse-glow' : ''}`}
                  />
                </div>
                <div className="min-w-0">
                  <p className="truncate text-sm font-semibold text-[var(--text-primary)]">
                    {agent.agent_id}
                  </p>
                  <div className="flex items-center gap-1.5 text-xs text-[var(--text-muted)]">
                    {agent.role ? <span className="truncate">{agent.role}</span> : <span>agent</span>}
                    {agent.trust_level !== null && (
                      <>
                        <span className="text-[var(--text-placeholder)]">·</span>
                        <span className="inline-flex items-center gap-1">
                          <Shield className="h-3 w-3" />
                          L{agent.trust_level}
                        </span>
                      </>
                    )}
                  </div>
                </div>
              </div>

              <div className="flex flex-col items-end gap-1">
                <span className={`rounded-full px-2 py-1 text-[10px] font-semibold uppercase tracking-wide ${
                  agent.status === 'online'
                    ? 'bg-[var(--status-success)]/10 text-[var(--status-success)]'
                    : agent.status === 'error'
                      ? 'bg-[var(--status-error)]/10 text-[var(--status-error)]'
                      : 'bg-[var(--bg-secondary)] text-[var(--text-muted)]'
                }`}>
                  {agent.status}
                </span>
                {label && (
                  <span className="rounded-full border border-[var(--border-default)] bg-[var(--bg-secondary)] px-2 py-0.5 text-[10px] font-medium text-[var(--text-muted)]">
                    {label}
                  </span>
                )}
              </div>
            </div>

            <div className="mt-3 flex items-end justify-between gap-3">
              <div className="flex items-end gap-1">
                {bars.map((active, idx) => (
                  <span
                    key={idx}
                    style={{
                      width: '9px',
                      height: `${14 + idx * 4}px`,
                      background: active
                        ? 'linear-gradient(180deg, rgba(217,90,30,0.95), rgba(217,90,30,0.35))'
                        : 'var(--border-default)',
                      opacity: active ? 1 : 0.5,
                    }}
                    className="rounded-full transition-all duration-300"
                  />
                ))}
              </div>
              <span className="inline-flex items-center gap-1 text-[11px] text-[var(--text-placeholder)]">
                <Radio className="h-3.5 w-3.5" />
                {agent.channels.length} channels
              </span>
            </div>

            {selected && (
              <div className="mt-3 inline-flex items-center gap-1 rounded-full bg-[var(--accent-primary)]/10 px-2 py-1 text-[10px] font-semibold uppercase tracking-wide text-[var(--accent-primary)]">
                <Sparkles className="h-3 w-3" />
                Workbench
              </div>
            )}
          </button>
        );
      })}
    </div>
  );
}
