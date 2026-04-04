import { Activity, BookMarked, BrainCircuit, Gauge, Layers3, Sparkles, Target, X } from 'lucide-react';
import type {
  ChatSessionInfo,
  ContextBudgetResponse,
  MemoryStatsResponse,
  PostTurnReportEvent,
} from '@/types/api';

interface MemoryPulseProps {
  stats: MemoryStatsResponse | null;
  budget: ContextBudgetResponse | null;
  lastReport: PostTurnReportEvent | null;
  session: ChatSessionInfo | null;
  agentLabel: string;
  onClose?: () => void;
}

function formatCount(value: number): string {
  return new Intl.NumberFormat().format(value);
}

function formatChars(value: number): string {
  return `${new Intl.NumberFormat().format(value)} chars`;
}

function allocationShare(value: number, total: number): number {
  if (total <= 0 || value <= 0) return 0;
  return Math.max(10, Math.round((value / total) * 100));
}

function latestLearningText(report: PostTurnReportEvent | null): string {
  if (!report) return 'No learning event yet for this agent in the current stream.';
  if (report.explicit_kind) return report.explicit_kind.replace(/_/g, ' ');
  if (report.explicit_mutation) return 'explicit mutation';
  if (report.reflection_started) return 'reflection cycle';
  if (report.consolidation_started) return 'memory consolidation';
  return 'passive turn processing';
}

function latestLearningTone(report: PostTurnReportEvent | null): string {
  if (!report) return 'text-[var(--text-muted)]';
  if (report.explicit_mutation) return 'text-[var(--accent-primary)]';
  if (report.reflection_started) return 'text-[var(--status-success)]';
  return 'text-[var(--text-primary)]';
}

export default function MemoryPulse({
  stats,
  budget,
  lastReport,
  session,
  agentLabel,
  onClose,
}: MemoryPulseProps) {
  const topCategories = stats?.by_category.slice(0, 4) ?? [];
  const totalEntries = stats?.total_entries ?? 0;
  const recallShare = allocationShare(
    budget?.recall_total_max_chars ?? 0,
    budget?.enrichment_total_max_chars ?? 0,
  );
  const skillsShare = allocationShare(
    budget?.skills_total_max_chars ?? 0,
    budget?.enrichment_total_max_chars ?? 0,
  );
  const entitiesShare = allocationShare(
    budget?.entities_total_max_chars ?? 0,
    budget?.enrichment_total_max_chars ?? 0,
  );

  return (
    <aside className="flex h-full flex-col overflow-hidden bg-[var(--bg-secondary)]">
      <div className="border-b border-[var(--border-default)] px-4 py-4">
        <div className="flex items-start justify-between gap-3">
          <div>
            <p className="text-[11px] font-semibold uppercase tracking-[0.24em] text-[var(--text-placeholder)]">
              Memory Pulse
            </p>
            <h2 className="mt-1 text-lg font-semibold text-[var(--text-primary)]">
              {agentLabel}
            </h2>
            <p className="mt-1 text-xs text-[var(--text-muted)]">
              Live memory surface, learning state, and context envelope.
            </p>
          </div>
          {onClose && (
            <button
              onClick={onClose}
              className="rounded-xl border border-[var(--border-default)] bg-[var(--bg-card)] p-2 text-[var(--text-muted)] transition-colors hover:text-[var(--text-primary)]"
              aria-label="Close memory pulse"
            >
              <X className="h-4 w-4" />
            </button>
          )}
        </div>
      </div>

      <div className="flex-1 space-y-4 overflow-y-auto px-4 py-4">
        <section className="glass-card overflow-hidden">
          <div className="border-b border-[var(--border-default)] px-4 py-3">
            <div className="flex items-center gap-2">
              <Sparkles className="h-4 w-4 text-[var(--accent-primary)]" />
              <h3 className="text-sm font-semibold text-[var(--text-primary)]">Latest Learning</h3>
            </div>
          </div>
          <div className="space-y-3 px-4 py-4">
            <div>
              <p className="text-[11px] uppercase tracking-[0.22em] text-[var(--text-placeholder)]">
                Trigger
              </p>
              <p className={`mt-1 text-sm font-medium ${latestLearningTone(lastReport)}`}>
                {lastReport?.signal ?? 'awaiting signal'}
              </p>
              <p className="mt-1 text-xs text-[var(--text-muted)]">
                {latestLearningText(lastReport)}
              </p>
            </div>
            <div className="flex flex-wrap gap-2">
              <span className={`rounded-full px-2.5 py-1 text-[10px] font-semibold uppercase tracking-wide ${
                lastReport?.explicit_mutation
                  ? 'bg-[var(--accent-primary)]/12 text-[var(--accent-primary)]'
                  : 'bg-[var(--bg-hover)] text-[var(--text-muted)]'
              }`}>
                explicit
              </span>
              <span className={`rounded-full px-2.5 py-1 text-[10px] font-semibold uppercase tracking-wide ${
                lastReport?.consolidation_started
                  ? 'bg-[var(--status-info)]/10 text-[var(--status-info)]'
                  : 'bg-[var(--bg-hover)] text-[var(--text-muted)]'
              }`}>
                consolidated
              </span>
              <span className={`rounded-full px-2.5 py-1 text-[10px] font-semibold uppercase tracking-wide ${
                lastReport?.reflection_started
                  ? 'bg-[var(--status-success)]/12 text-[var(--status-success)]'
                  : 'bg-[var(--bg-hover)] text-[var(--text-muted)]'
              }`}>
                reflected
              </span>
            </div>
          </div>
        </section>

        <section className="glass-card overflow-hidden">
          <div className="border-b border-[var(--border-default)] px-4 py-3">
            <div className="flex items-center gap-2">
              <BrainCircuit className="h-4 w-4 text-[var(--accent-primary)]" />
              <h3 className="text-sm font-semibold text-[var(--text-primary)]">Memory Surface</h3>
            </div>
          </div>
          <div className="space-y-4 px-4 py-4">
            <div className="grid grid-cols-2 gap-3">
              <div className="rounded-2xl border border-[var(--border-default)] bg-[var(--bg-card)] px-3 py-3">
                <p className="text-[10px] uppercase tracking-[0.2em] text-[var(--text-placeholder)]">Entries</p>
                <p className="mt-1 text-lg font-semibold text-[var(--text-primary)]">{formatCount(totalEntries)}</p>
              </div>
              <div className="rounded-2xl border border-[var(--border-default)] bg-[var(--bg-card)] px-3 py-3">
                <p className="text-[10px] uppercase tracking-[0.2em] text-[var(--text-placeholder)]">Skills</p>
                <p className="mt-1 text-lg font-semibold text-[var(--text-primary)]">{formatCount(stats?.skills ?? 0)}</p>
              </div>
              <div className="rounded-2xl border border-[var(--border-default)] bg-[var(--bg-card)] px-3 py-3">
                <p className="text-[10px] uppercase tracking-[0.2em] text-[var(--text-placeholder)]">Entities</p>
                <p className="mt-1 text-lg font-semibold text-[var(--text-primary)]">{formatCount(stats?.entities ?? 0)}</p>
              </div>
              <div className="rounded-2xl border border-[var(--border-default)] bg-[var(--bg-card)] px-3 py-3">
                <p className="text-[10px] uppercase tracking-[0.2em] text-[var(--text-placeholder)]">Reflections</p>
                <p className="mt-1 text-lg font-semibold text-[var(--text-primary)]">{formatCount(stats?.reflections ?? 0)}</p>
              </div>
            </div>

            <div className="space-y-2">
              <div className="flex items-center gap-2">
                <Layers3 className="h-3.5 w-3.5 text-[var(--text-muted)]" />
                <p className="text-xs font-medium text-[var(--text-secondary)]">Top Categories</p>
              </div>
              {topCategories.length === 0 ? (
                <p className="text-xs text-[var(--text-muted)]">No category stats available yet.</p>
              ) : (
                topCategories.map((category) => {
                  const width = totalEntries > 0 ? Math.max(8, Math.round((category.count / totalEntries) * 100)) : 0;
                  return (
                    <div key={category.category} className="space-y-1">
                      <div className="flex items-center justify-between gap-3 text-xs">
                        <span className="capitalize text-[var(--text-secondary)]">{category.category}</span>
                        <span className="text-[var(--text-muted)]">{formatCount(category.count)}</span>
                      </div>
                      <div className="h-2 overflow-hidden rounded-full bg-[var(--bg-hover)]">
                        <div
                          className="h-full rounded-full"
                          style={{
                            width: `${width}%`,
                            background: 'linear-gradient(90deg, var(--accent-primary), rgba(217, 90, 30, 0.45))',
                          }}
                        />
                      </div>
                    </div>
                  );
                })
              )}
            </div>

            <div className="space-y-2">
              <div className="flex items-center gap-2">
                <BookMarked className="h-3.5 w-3.5 text-[var(--text-muted)]" />
                <p className="text-xs font-medium text-[var(--text-secondary)]">Core Blocks</p>
              </div>
              {stats?.core_blocks.length ? (
                <div className="space-y-2">
                  {stats.core_blocks.slice(0, 4).map((block) => (
                    <div
                      key={block.label}
                      className="rounded-2xl border border-[var(--border-default)] bg-[var(--bg-card)] px-3 py-2"
                    >
                      <div className="flex items-center justify-between gap-3">
                        <span className="text-xs font-medium capitalize text-[var(--text-primary)]">
                          {block.label.replace(/_/g, ' ')}
                        </span>
                        <span className="text-[11px] text-[var(--text-muted)]">{formatChars(block.chars)}</span>
                      </div>
                    </div>
                  ))}
                </div>
              ) : (
                <p className="text-xs text-[var(--text-muted)]">Core blocks have not been surfaced yet.</p>
              )}
            </div>
          </div>
        </section>

        <section className="glass-card overflow-hidden">
          <div className="border-b border-[var(--border-default)] px-4 py-3">
            <div className="flex items-center gap-2">
              <Gauge className="h-4 w-4 text-[var(--accent-primary)]" />
              <h3 className="text-sm font-semibold text-[var(--text-primary)]">Context Budget</h3>
            </div>
          </div>
          <div className="space-y-4 px-4 py-4">
            {budget ? (
              <>
                <div className="rounded-2xl border border-[var(--border-default)] bg-[var(--bg-card)] px-3 py-3">
                  <div className="flex items-center justify-between gap-2 text-xs">
                    <span className="text-[var(--text-secondary)]">Continuation Mode</span>
                    <span className="font-semibold uppercase tracking-wide text-[var(--accent-primary)]">
                      {budget.continuation_policy}
                    </span>
                  </div>
                  <p className="mt-2 text-[11px] text-[var(--text-muted)]">
                    Total enrichment envelope: {formatChars(budget.enrichment_total_max_chars)}
                  </p>
                </div>

                {[
                  {
                    label: 'Recall',
                    share: recallShare,
                    meta: `${budget.recall_max_entries} entries · ${formatChars(budget.recall_total_max_chars)}`,
                  },
                  {
                    label: 'Skills',
                    share: skillsShare,
                    meta: `${budget.skills_max_count} items · ${formatChars(budget.skills_total_max_chars)}`,
                  },
                  {
                    label: 'Entities',
                    share: entitiesShare,
                    meta: `${budget.entities_max_count} items · ${formatChars(budget.entities_total_max_chars)}`,
                  },
                ].map((item) => (
                  <div key={item.label} className="space-y-1.5">
                    <div className="flex items-center justify-between gap-3 text-xs">
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

                <div className="rounded-2xl border border-[var(--border-default)] bg-[var(--bg-card)] px-3 py-3 text-xs text-[var(--text-muted)]">
                  Min relevance score: <span className="font-medium text-[var(--text-primary)]">{budget.min_relevance_score.toFixed(2)}</span>
                </div>
              </>
            ) : (
              <p className="text-xs text-[var(--text-muted)]">Context budget is not available for this agent yet.</p>
            )}
          </div>
        </section>

        <section className="glass-card overflow-hidden">
          <div className="border-b border-[var(--border-default)] px-4 py-3">
            <div className="flex items-center gap-2">
              <Target className="h-4 w-4 text-[var(--accent-primary)]" />
              <h3 className="text-sm font-semibold text-[var(--text-primary)]">Session Lens</h3>
            </div>
          </div>
          <div className="space-y-3 px-4 py-4">
            <div className="rounded-2xl border border-[var(--border-default)] bg-[var(--bg-card)] px-3 py-3">
              <p className="text-[10px] uppercase tracking-[0.2em] text-[var(--text-placeholder)]">Current Goal</p>
              <p className="mt-1 text-sm text-[var(--text-primary)]">
                {session?.current_goal ?? 'No explicit goal captured for this session yet.'}
              </p>
            </div>
            <div className="rounded-2xl border border-[var(--border-default)] bg-[var(--bg-card)] px-3 py-3">
              <p className="text-[10px] uppercase tracking-[0.2em] text-[var(--text-placeholder)]">Summary</p>
              <p className="mt-1 text-sm text-[var(--text-primary)]">
                {session?.session_summary ?? session?.preview ?? 'The transcript is still building its own shape.'}
              </p>
            </div>
            <div className="flex items-center justify-between gap-3 rounded-2xl border border-[var(--border-default)] bg-[var(--bg-card)] px-3 py-3 text-xs">
              <span className="inline-flex items-center gap-1.5 text-[var(--text-secondary)]">
                <Activity className="h-3.5 w-3.5 text-[var(--accent-primary)]" />
                Active run
              </span>
              <span className={session?.has_active_run ? 'text-[var(--status-success)]' : 'text-[var(--text-muted)]'}>
                {session?.has_active_run ? 'in progress' : 'idle'}
              </span>
            </div>
          </div>
        </section>
      </div>
    </aside>
  );
}
