import { useState, useEffect, useMemo } from 'react';
import { useNavigate } from 'react-router-dom';
import {
  Cpu,
  Clock,
  Globe,
  Database,
  Activity,
  DollarSign,
  Radio,
  BrainCircuit,
  Sparkles,
  Gauge,
  ArrowRight,
  Network,
  Shield,
} from 'lucide-react';
import type {
  StatusResponse,
  CostSummary,
  MemoryStatsResponse,
  ContextBudgetResponse,
  PostTurnReportEvent,
} from '@/types/api';
import { getStatus, getCost, getAgents, getMemoryStats, getContextBudget, type AgentEntry } from '@/lib/api';
import { t } from '@/lib/i18n';
import { useSSE } from '@/hooks/useSSE';

function formatUptime(seconds: number): string {
  const d = Math.floor(seconds / 86400);
  const h = Math.floor((seconds % 86400) / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  if (d > 0) return `${d}d ${h}h ${m}m`;
  if (h > 0) return `${h}h ${m}m`;
  return `${m}m`;
}

function formatUSD(value: number): string {
  return `$${value.toFixed(4)}`;
}

function formatChars(value: number): string {
  return `${new Intl.NumberFormat().format(value)} chars`;
}

function healthColor(status: string): string {
  switch (status.toLowerCase()) {
    case 'ok':
    case 'healthy':
      return 'bg-[var(--status-success)]';
    case 'warn':
    case 'warning':
    case 'degraded':
      return 'bg-[#C9872C]';
    default:
      return 'bg-[var(--status-error)]';
  }
}

function healthBorder(status: string): string {
  switch (status.toLowerCase()) {
    case 'ok':
    case 'healthy':
      return 'border-[#2D8A4E]/30';
    case 'warn':
    case 'warning':
    case 'degraded':
      return 'border-[#C9872C]/30';
    default:
      return 'border-[#C73E3E]/30';
  }
}

function budgetShare(value: number, total: number): number {
  if (total <= 0 || value <= 0) return 0;
  return Math.max(8, Math.round((value / total) * 100));
}

function learningTone(event: PostTurnReportEvent): string {
  if (event.explicit_mutation) return 'text-[var(--accent-primary)]';
  if (event.reflection_started) return 'text-[var(--status-success)]';
  if (event.consolidation_started) return 'text-[var(--status-info)]';
  return 'text-[var(--text-primary)]';
}

function learningLabel(event: PostTurnReportEvent): string {
  if (event.explicit_kind) return event.explicit_kind.replace(/_/g, ' ');
  if (event.explicit_mutation) return 'explicit mutation';
  if (event.reflection_started) return 'reflection';
  if (event.consolidation_started) return 'consolidation';
  return 'passive turn';
}

function OverviewCard({
  icon: Icon,
  label,
  value,
  sub,
  color,
  bg,
}: {
  icon: React.ComponentType<{ className?: string }>;
  label: string;
  value: string;
  sub: string;
  color: string;
  bg: string;
}) {
  return (
    <div className="glass-card p-5 animate-slide-in-up">
        <div className="mb-3 flex items-center gap-3">
        <div className="rounded-xl p-2" style={{ background: bg, color }}>
          <Icon className="h-5 w-5" />
        </div>
        <span className="text-xs font-medium uppercase tracking-wider text-[var(--text-muted)]">{label}</span>
      </div>
      <p className="truncate text-lg font-semibold capitalize text-[var(--text-primary)]">{value}</p>
      <p className="truncate text-sm text-[var(--text-muted)]">{sub}</p>
    </div>
  );
}

export default function Dashboard() {
  const navigate = useNavigate();
  const [status, setStatus] = useState<StatusResponse | null>(null);
  const [cost, setCost] = useState<CostSummary | null>(null);
  const [memoryStats, setMemoryStats] = useState<MemoryStatsResponse | null>(null);
  const [contextBudget, setContextBudget] = useState<ContextBudgetResponse | null>(null);
  const [agents, setAgents] = useState<AgentEntry[]>([]);
  const [error, setError] = useState<string | null>(null);
  const { events } = useSSE({
    filterTypes: ['post_turn_report'],
    maxEvents: 20,
  });

  useEffect(() => {
    Promise.all([getStatus(), getCost(), getMemoryStats(), getContextBudget(), getAgents()])
      .then(([s, c, memory, budget, fleetAgents]) => {
        setStatus(s);
        setCost(c);
        setMemoryStats(memory);
        setContextBudget(budget);
        setAgents(fleetAgents);
      })
      .catch((err) => setError(err.message));
  }, []);

  const learningEvents = useMemo(
    () =>
      [...events]
        .reverse()
        .filter((event): event is PostTurnReportEvent => event.type === 'post_turn_report')
        .slice(0, 5),
    [events],
  );

  if (error) {
    return (
      <div className="p-6 animate-fade-in">
        <div className="rounded-xl bg-[var(--status-error)]/10 border border-[#C73E3E]/20 p-4 text-[#C73E3E]">
          {t('dashboard.load_error')}: {error}
        </div>
      </div>
    );
  }

  if (!status || !cost || !memoryStats || !contextBudget) {
    return (
      <div className="flex items-center justify-center h-64">
        <div className="h-8 w-8 border-2 border-[var(--accent-primary)]/20 border-t-[#D95A1E] rounded-full animate-spin" />
      </div>
    );
  }

  const maxCost = Math.max(cost.session_cost_usd, cost.daily_cost_usd, cost.monthly_cost_usd, 0.001);
  const onlineAgents = agents.filter((agent) => agent.status === 'online').length;
  const recallShare = budgetShare(contextBudget.recall_total_max_chars, contextBudget.enrichment_total_max_chars);
  const skillsShare = budgetShare(contextBudget.skills_total_max_chars, contextBudget.enrichment_total_max_chars);
  const entitiesShare = budgetShare(contextBudget.entities_total_max_chars, contextBudget.enrichment_total_max_chars);

  return (
    <div className="space-y-6 p-6 animate-fade-in">
      <div className="relative overflow-hidden rounded-[28px] border border-[var(--border-default)] bg-[linear-gradient(135deg,var(--glow-primary),transparent_35%),var(--bg-card)] px-6 py-6">
        <div className="absolute inset-x-10 top-0 h-px bg-gradient-to-r from-transparent via-[var(--accent-primary)]/70 to-transparent" />
        <div className="flex flex-col gap-5 xl:flex-row xl:items-end xl:justify-between">
          <div>
            <p className="text-[11px] font-semibold uppercase tracking-[0.28em] text-[var(--text-placeholder)]">
              Runtime Overview
            </p>
            <div className="mt-2 flex flex-wrap items-center gap-2">
              <h1 className="text-3xl font-semibold tracking-tight text-[var(--text-primary)]">
                SynapseClaw Control Room
              </h1>
              <span className="rounded-full bg-[var(--accent-primary)]/10 px-2.5 py-1 text-[10px] font-semibold uppercase tracking-wide text-[var(--accent-primary)]">
                {memoryStats.agent_id}
              </span>
            </div>
            <p className="mt-2 max-w-2xl text-sm text-[var(--text-muted)]">
              Fleet status, memory health, learning activity, and budget posture in one place.
            </p>
          </div>

          <div className="flex flex-wrap gap-2">
            <button
              onClick={() => navigate('/agents')}
              className="btn-primary inline-flex items-center gap-2 px-4 py-2 text-sm"
            >
              <Sparkles className="h-4 w-4" />
              Open Workbench
            </button>
            <button
              onClick={() => navigate('/memory')}
              className="btn-secondary inline-flex items-center gap-2 px-4 py-2 text-sm"
            >
              <BrainCircuit className="h-4 w-4" />
              Memory Studio
            </button>
            <button
              onClick={() => navigate('/ipc/fleet')}
              className="btn-secondary inline-flex items-center gap-2 px-4 py-2 text-sm"
            >
              <Network className="h-4 w-4" />
              Fleet View
            </button>
          </div>
        </div>
      </div>

      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-4 stagger-children">
        <OverviewCard
          icon={Cpu}
          color="var(--accent-primary)"
          bg="var(--glow-secondary)"
          label={t('dashboard.provider_model')}
          value={status.provider ?? 'Unknown'}
          sub={status.model}
        />
        <OverviewCard
          icon={Clock}
          color="var(--status-success)"
          bg="var(--glow-secondary)"
          label={t('dashboard.uptime')}
          value={formatUptime(status.uptime_seconds)}
          sub={t('dashboard.since_last_restart')}
        />
        <OverviewCard
          icon={Database}
          color="var(--status-warning)"
          bg="var(--glow-secondary)"
          label="Memory Surface"
          value={memoryStats.total_entries.toLocaleString()}
          sub={`${memoryStats.skills} skills · ${memoryStats.reflections} reflections`}
        />
        <OverviewCard
          icon={Radio}
          color="var(--text-muted)"
          bg="var(--bg-secondary)"
          label="Fleet Pulse"
          value={`${onlineAgents}/${agents.length + 1}`}
          sub="online agents across broker scope"
        />
      </div>

      <div className="grid grid-cols-1 gap-6 xl:grid-cols-[1.05fr_0.95fr_0.9fr] stagger-children">
        <div className="glass-card p-5 animate-slide-in-up">
          <div className="mb-5 flex items-center gap-2">
            <DollarSign className="h-5 w-5 text-[var(--accent-primary)]" />
            <h2 className="text-sm font-semibold uppercase tracking-wider text-[var(--text-primary)]">{t('dashboard.cost_overview')}</h2>
          </div>
          <div className="space-y-4">
            {[
              { label: t('dashboard.session_label'), value: cost.session_cost_usd, color: 'var(--accent-primary)' },
              { label: t('dashboard.daily_label'), value: cost.daily_cost_usd, color: 'var(--status-success)' },
              { label: t('dashboard.monthly_label'), value: cost.monthly_cost_usd, color: 'var(--text-muted)' },
            ].map(({ label, value, color }) => (
              <div key={label}>
                <div className="mb-1.5 flex justify-between text-sm">
                  <span className="text-[var(--text-muted)]">{label}</span>
                  <span className="font-mono font-medium text-[var(--text-primary)]">{formatUSD(value)}</span>
                </div>
                <div className="h-1.5 w-full overflow-hidden rounded-full bg-[var(--bg-hover)]">
                  <div
                    className="progress-bar-animated h-full rounded-full transition-all duration-700 ease-out"
                    style={{ width: `${Math.max((value / maxCost) * 100, 2)}%`, background: color }}
                  />
                </div>
              </div>
            ))}
          </div>
          <div className="mt-5 flex justify-between border-t border-[var(--border-default)] pt-4 text-sm">
            <span className="text-[var(--text-muted)]">{t('dashboard.total_tokens_label')}</span>
            <span className="font-mono text-[var(--text-primary)]">{cost.total_tokens.toLocaleString()}</span>
          </div>
          <div className="mt-1 flex justify-between text-sm">
            <span className="text-[var(--text-muted)]">{t('dashboard.requests_label')}</span>
            <span className="font-mono text-[var(--text-primary)]">{cost.request_count.toLocaleString()}</span>
          </div>
        </div>

        <div className="glass-card p-5 animate-slide-in-up">
          <div className="mb-5 flex items-center gap-2">
            <Gauge className="h-5 w-5 text-[var(--accent-primary)]" />
            <h2 className="text-sm font-semibold uppercase tracking-wider text-[var(--text-primary)]">Memory Health</h2>
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div className="rounded-2xl border border-[var(--border-default)] bg-[var(--bg-card)] px-3 py-3">
              <p className="text-[10px] uppercase tracking-[0.22em] text-[var(--text-placeholder)]">Entities</p>
              <p className="mt-1 text-xl font-semibold text-[var(--text-primary)]">{memoryStats.entities}</p>
            </div>
            <div className="rounded-2xl border border-[var(--border-default)] bg-[var(--bg-card)] px-3 py-3">
              <p className="text-[10px] uppercase tracking-[0.22em] text-[var(--text-placeholder)]">Core Blocks</p>
              <p className="mt-1 text-xl font-semibold text-[var(--text-primary)]">{memoryStats.core_blocks.length}</p>
            </div>
          </div>

          <div className="mt-5 space-y-3">
            {[
              {
                label: 'Recall',
                share: recallShare,
                meta: `${contextBudget.recall_max_entries} entries · ${formatChars(contextBudget.recall_total_max_chars)}`,
              },
              {
                label: 'Skills',
                share: skillsShare,
                meta: `${contextBudget.skills_max_count} items · ${formatChars(contextBudget.skills_total_max_chars)}`,
              },
              {
                label: 'Entities',
                share: entitiesShare,
                meta: `${contextBudget.entities_max_count} items · ${formatChars(contextBudget.entities_total_max_chars)}`,
              },
            ].map((item) => (
              <div key={item.label} className="space-y-1.5">
                <div className="flex items-center justify-between gap-3 text-sm">
                  <span className="font-medium text-[var(--text-secondary)]">{item.label}</span>
                  <span className="text-[var(--text-muted)]">{item.meta}</span>
                </div>
                <div className="h-2 overflow-hidden rounded-full bg-[var(--bg-hover)]">
                  <div
                    className="h-full rounded-full"
                    style={{
                      width: `${item.share}%`,
                      background: 'linear-gradient(90deg, var(--accent-primary), rgba(217, 90, 30, 0.45))',
                    }}
                  />
                </div>
              </div>
            ))}
          </div>

          <div className="mt-5 rounded-2xl border border-[var(--border-default)] bg-[var(--bg-card)] px-4 py-3 text-sm">
            <div className="flex items-center justify-between gap-3">
              <span className="text-[var(--text-muted)]">Continuation mode</span>
              <span className="font-semibold uppercase tracking-wide text-[var(--accent-primary)]">
                {contextBudget.continuation_policy}
              </span>
            </div>
          </div>
        </div>

        <div className="glass-card p-5 animate-slide-in-up">
          <div className="mb-5 flex items-center gap-2">
            <Sparkles className="h-5 w-5 text-[var(--accent-primary)]" />
            <h2 className="text-sm font-semibold uppercase tracking-wider text-[var(--text-primary)]">Learning Feed</h2>
          </div>
          {learningEvents.length === 0 ? (
            <div className="rounded-2xl border border-[var(--border-default)] bg-[var(--bg-card)] px-4 py-5 text-sm text-[var(--text-muted)]">
              No learning events have reached the live stream yet.
            </div>
          ) : (
            <div className="space-y-3">
              {learningEvents.map((event, idx) => (
                <div
                  key={`${event.agent_id}-${event.timestamp ?? idx}`}
                  className="rounded-2xl border border-[var(--border-default)] bg-[var(--bg-card)] px-4 py-3"
                >
                  <div className="flex items-center justify-between gap-3">
                    <span className="text-sm font-medium text-[var(--text-primary)]">{event.agent_id}</span>
                    <span className={`text-xs font-semibold uppercase tracking-wide ${learningTone(event)}`}>
                      {learningLabel(event)}
                    </span>
                  </div>
                  <p className="mt-1 text-xs text-[var(--text-muted)]">
                    signal: {event.signal}
                  </p>
                  <div className="mt-2 flex flex-wrap gap-2">
                    {event.explicit_mutation && (
                      <span className="rounded-full bg-[var(--accent-primary)]/10 px-2 py-0.5 text-[10px] font-semibold uppercase tracking-wide text-[var(--accent-primary)]">
                        explicit
                      </span>
                    )}
                    {event.consolidation_started && (
                      <span className="rounded-full bg-[var(--status-info)]/10 px-2 py-0.5 text-[10px] font-semibold uppercase tracking-wide text-[var(--status-info)]">
                        consolidated
                      </span>
                    )}
                    {event.reflection_started && (
                      <span className="rounded-full bg-[var(--status-success)]/10 px-2 py-0.5 text-[10px] font-semibold uppercase tracking-wide text-[var(--status-success)]">
                        reflected
                      </span>
                    )}
                  </div>
                </div>
              ))}
            </div>
          )}
        </div>
      </div>

      <div className="grid grid-cols-1 gap-6 xl:grid-cols-[1fr_1fr] stagger-children">
        <div className="glass-card p-5 animate-slide-in-up">
          <div className="mb-5 flex items-center gap-2">
            <Radio className="h-5 w-5 text-[var(--accent-primary)]" />
            <h2 className="text-sm font-semibold uppercase tracking-wider text-[var(--text-primary)]">{t('dashboard.active_channels')}</h2>
          </div>
          <div className="space-y-2">
            {Object.entries(status.channels).length === 0 ? (
              <p className="text-sm text-[var(--text-placeholder)]">{t('dashboard.no_channels')}</p>
            ) : (
              Object.entries(status.channels).map(([name, active]) => (
                <div
                  key={name}
                  className="flex items-center justify-between rounded-xl px-3 py-2.5 transition-all duration-300 hover:bg-[var(--bg-hover)]"
                >
                  <span className="text-sm font-medium capitalize text-[var(--text-primary)]">{name}</span>
                  <div className="flex items-center gap-2">
                    <span
                      className={`inline-block h-2 w-2 rounded-full ${
                        active ? 'bg-[var(--status-success)]' : 'bg-[var(--text-placeholder)]'
                      }`}
                    />
                    <span className="text-xs text-[var(--text-muted)]">
                      {active ? t('dashboard.active') : t('dashboard.inactive')}
                    </span>
                  </div>
                </div>
              ))
            )}
          </div>
        </div>

        <div className="glass-card p-5 animate-slide-in-up">
          <div className="mb-5 flex items-center gap-2">
            <Network className="h-5 w-5 text-[var(--accent-primary)]" />
            <h2 className="text-sm font-semibold uppercase tracking-wider text-[var(--text-primary)]">Fleet Readiness</h2>
          </div>
          <div className="space-y-3">
            <button
              onClick={() => navigate('/ipc/fleet')}
              className="flex w-full items-center justify-between rounded-2xl border border-[var(--border-default)] bg-[var(--bg-card)] px-4 py-3 text-left transition-colors hover:border-[var(--accent-primary)]/30"
            >
              <div>
                <p className="text-sm font-medium text-[var(--text-primary)]">Fleet map</p>
                <p className="mt-1 text-xs text-[var(--text-muted)]">{onlineAgents} online remote agents, {agents.length} registered peers</p>
              </div>
              <ArrowRight className="h-4 w-4 text-[var(--text-muted)]" />
            </button>

            {agents.slice(0, 4).map((agent) => (
              <button
                key={agent.agent_id}
                onClick={() => navigate(`/agents?agent=${encodeURIComponent(agent.agent_id)}`)}
                className="flex w-full items-center justify-between rounded-2xl border border-[var(--border-default)] bg-[var(--bg-card)] px-4 py-3 text-left transition-colors hover:border-[var(--accent-primary)]/30"
              >
                <div>
                  <div className="flex items-center gap-2">
                    <span className="text-sm font-medium text-[var(--text-primary)]">{agent.agent_id}</span>
                    {agent.trust_level !== null && (
                      <span className="inline-flex items-center gap-1 rounded-full bg-[var(--bg-hover)] px-2 py-0.5 text-[10px] font-semibold uppercase tracking-wide text-[var(--text-muted)]">
                        <Shield className="h-3 w-3" />
                        L{agent.trust_level}
                      </span>
                    )}
                  </div>
                  <p className="mt-1 text-xs text-[var(--text-muted)]">
                    {agent.role ?? 'agent'} · {agent.channels.length} channels
                  </p>
                </div>
                <span className={`rounded-full px-2 py-1 text-[10px] font-semibold uppercase tracking-wide ${
                  agent.status === 'online'
                    ? 'bg-[var(--status-success)]/10 text-[var(--status-success)]'
                    : agent.status === 'error'
                      ? 'bg-[var(--status-error)]/10 text-[var(--status-error)]'
                      : 'bg-[var(--bg-hover)] text-[var(--text-muted)]'
                }`}>
                  {agent.status}
                </span>
              </button>
            ))}
          </div>
        </div>
      </div>

      <div className="glass-card p-5 animate-slide-in-up">
        <div className="mb-5 flex items-center gap-2">
          <Activity className="h-5 w-5 text-[var(--accent-primary)]" />
          <h2 className="text-sm font-semibold uppercase tracking-wider text-[var(--text-primary)]">{t('dashboard.component_health')}</h2>
        </div>
        <div className="grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-4">
          {Object.entries(status.health.components).length === 0 ? (
            <p className="col-span-4 text-sm text-[var(--text-placeholder)]">{t('dashboard.no_components')}</p>
          ) : (
            Object.entries(status.health.components).map(([name, comp]) => (
              <div
                key={name}
                className={`rounded-xl border p-3 transition-all duration-300 hover:scale-[1.02] ${healthBorder(comp.status)}`}
              >
                <div className="mb-1 flex items-center gap-2">
                  <span className={`inline-block h-2 w-2 rounded-full ${healthColor(comp.status)}`} />
                  <span className="truncate text-sm font-medium capitalize text-[var(--text-primary)]">
                    {name}
                  </span>
                </div>
                <p className="text-xs capitalize text-[var(--text-muted)]">{comp.status}</p>
                {comp.restart_count > 0 && (
                  <p className="mt-1 text-xs text-[#C9872C]">
                    {t('dashboard.restarts')}: {comp.restart_count}
                  </p>
                )}
              </div>
            ))
          )}
        </div>
      </div>

      <div className="glass-card p-5 animate-slide-in-up">
        <div className="mb-5 flex items-center gap-2">
          <Globe className="h-5 w-5 text-[var(--accent-primary)]" />
          <h2 className="text-sm font-semibold uppercase tracking-wider text-[var(--text-primary)]">Runtime Footing</h2>
        </div>
        <div className="grid grid-cols-1 gap-3 md:grid-cols-3">
          <div className="rounded-2xl border border-[var(--border-default)] bg-[var(--bg-card)] px-4 py-4">
            <p className="text-[10px] uppercase tracking-[0.22em] text-[var(--text-placeholder)]">{t('dashboard.gateway_port')}</p>
            <p className="mt-2 text-xl font-semibold text-[var(--text-primary)]">:{status.gateway_port}</p>
          </div>
          <div className="rounded-2xl border border-[var(--border-default)] bg-[var(--bg-card)] px-4 py-4">
            <p className="text-[10px] uppercase tracking-[0.22em] text-[var(--text-placeholder)]">{t('dashboard.memory_backend')}</p>
            <p className="mt-2 text-xl font-semibold capitalize text-[var(--text-primary)]">{status.memory_backend}</p>
          </div>
          <div className="rounded-2xl border border-[var(--border-default)] bg-[var(--bg-card)] px-4 py-4">
            <p className="text-[10px] uppercase tracking-[0.22em] text-[var(--text-placeholder)]">{t('dashboard.paired')}</p>
            <p className="mt-2 text-xl font-semibold text-[var(--text-primary)]">
              {status.paired ? t('dashboard.paired_yes') : t('dashboard.paired_no')}
            </p>
          </div>
        </div>
      </div>
    </div>
  );
}
