import type { ReactNode } from 'react';
import { Activity, BookMarked, BrainCircuit, Gauge, Layers3, Sparkles, Target, X } from 'lucide-react';
import type {
  ChatSessionInfo,
  ContextBudgetResponse,
  MemoryStatsResponse,
  MemoryProjectionsResponse,
  PostTurnReportEvent,
} from '@/types/api';

interface MemoryPulseProps {
  stats: MemoryStatsResponse | null;
  budget: ContextBudgetResponse | null;
  projections: MemoryProjectionsResponse | null;
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

function formatLabel(value: string): string {
  return value.replace(/[_-]+/g, ' ');
}

function previewProjection(text: string | null | undefined, maxLines: number = 4): string | null {
  if (!text?.trim()) return null;
  const lines = text
    .trim()
    .split('\n')
    .filter((line) => line.trim().length > 0);
  if (lines.length <= maxLines) return lines.join('\n');
  return `${lines.slice(0, maxLines).join('\n')}\n…`;
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

function TinyBadge({ children }: { children: ReactNode }) {
  return (
    <span className="inline-flex items-center rounded-full border border-[var(--border-default)] bg-[var(--glow-secondary)] px-2.5 py-1 text-[10px] font-semibold uppercase tracking-wide text-[var(--text-muted)]">
      {children}
    </span>
  );
}

export default function MemoryPulse({
  stats,
  budget,
  projections,
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
  const nearbyShare = allocationShare(
    (budget?.nearby_max_entries ?? 0) * Math.min(160, budget?.recall_entry_max_chars ?? 0),
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
  const maintenanceReasons = projections?.learning_maintenance_plan?.reasons ?? [];
  const maintenanceActionCount = [
    projections?.learning_maintenance_plan?.run_importance_decay,
    projections?.learning_maintenance_plan?.run_gc,
    projections?.learning_maintenance_plan?.run_run_recipe_review,
    projections?.learning_maintenance_plan?.run_precedent_compaction,
    projections?.learning_maintenance_plan?.run_failure_pattern_compaction,
    projections?.learning_maintenance_plan?.run_skill_review,
    projections?.learning_maintenance_plan?.run_prompt_optimization,
  ].filter(Boolean).length;
  const topEffectiveSkills = projections?.effective_skills.slice(0, 3) ?? [];
  const workingStatePreview = previewProjection(projections?.working_state?.projection, 5);
  const profilePreview = previewProjection(projections?.current_user_profile?.projection, 5);

  return (
    <aside className="flex h-full flex-col overflow-hidden bg-[var(--bg-secondary)]">
      <div className="relative overflow-hidden border-b border-[var(--border-default)] bg-[radial-gradient(circle_at_top_left,rgba(217,90,30,0.18),transparent_42%),linear-gradient(180deg,rgba(255,252,248,0.96),rgba(255,247,240,0.92))] px-4 py-4">
        <div className="absolute inset-x-8 top-0 h-px bg-gradient-to-r from-transparent via-[var(--accent-primary)]/70 to-transparent" />
        <div className="relative flex items-start justify-between gap-3">
          <div>
            <p className="text-[11px] font-semibold uppercase tracking-[0.24em] text-[var(--text-placeholder)]">
              Atlas Memoriae
            </p>
            <h2 className="mt-1 text-lg font-semibold text-[var(--text-primary)]">
              {agentLabel}
            </h2>
            <p className="mt-1 text-xs text-[var(--text-muted)]">
              Live cortical pulse for learning, working state, and prompt budget.
            </p>
            <div className="mt-3 flex flex-wrap gap-1.5">
              <TinyBadge>Praefrontalis</TinyBadge>
              <TinyBadge>Hippocampus</TinyBadge>
              <TinyBadge>Amygdala</TinyBadge>
            </div>
          </div>
          {onClose && (
            <button
              onClick={onClose}
              className="rounded-xl border border-[var(--border-default)] bg-[var(--bg-card)]/80 p-2 text-[var(--text-muted)] transition-colors hover:text-[var(--text-primary)]"
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
              <div>
                <p className="text-[10px] font-semibold uppercase tracking-[0.24em] text-[var(--text-placeholder)]">
                  Amygdala
                </p>
                <h3 className="text-sm font-semibold text-[var(--text-primary)]">Learning Pulse</h3>
              </div>
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
            <div className="grid grid-cols-3 gap-2">
              <div className="rounded-2xl border border-[var(--border-default)] bg-[var(--bg-card)] px-3 py-3">
                <p className="text-[10px] uppercase tracking-[0.18em] text-[var(--text-placeholder)]">Conflicts</p>
                <p className="mt-1 text-base font-semibold text-[var(--text-primary)]">
                  {projections?.procedural_contradictions.length ?? 0}
                </p>
              </div>
              <div className="rounded-2xl border border-[var(--border-default)] bg-[var(--bg-card)] px-3 py-3">
                <p className="text-[10px] uppercase tracking-[0.18em] text-[var(--text-placeholder)]">Actions</p>
                <p className="mt-1 text-base font-semibold text-[var(--text-primary)]">
                  {maintenanceActionCount}
                </p>
              </div>
              <div className="rounded-2xl border border-[var(--border-default)] bg-[var(--bg-card)] px-3 py-3">
                <p className="text-[10px] uppercase tracking-[0.18em] text-[var(--text-placeholder)]">Skills</p>
                <p className="mt-1 text-base font-semibold text-[var(--text-primary)]">
                  {projections?.effective_skills.length ?? 0}
                </p>
              </div>
            </div>
            {maintenanceReasons.length > 0 && (
              <div className="flex flex-wrap gap-1.5">
                {maintenanceReasons.slice(0, 4).map((reason) => (
                  <TinyBadge key={reason}>{formatLabel(reason)}</TinyBadge>
                ))}
              </div>
            )}
          </div>
        </section>

        <section className="glass-card overflow-hidden">
          <div className="border-b border-[var(--border-default)] px-4 py-3">
            <div className="flex items-center gap-2">
              <BookMarked className="h-4 w-4 text-[var(--accent-primary)]" />
              <div>
                <p className="text-[10px] font-semibold uppercase tracking-[0.24em] text-[var(--text-placeholder)]">
                  Praefrontalis
                </p>
                <h3 className="text-sm font-semibold text-[var(--text-primary)]">Working State</h3>
              </div>
            </div>
          </div>
          <div className="space-y-3 px-4 py-4">
            <div className="rounded-2xl border border-[var(--border-default)] bg-[var(--bg-card)] px-3 py-3">
              <p className="text-[10px] uppercase tracking-[0.2em] text-[var(--text-placeholder)]">State projection</p>
              <pre className="mt-2 whitespace-pre-wrap break-words font-mono text-[11px] leading-5 text-[var(--text-secondary)]">
                {workingStatePreview ?? 'No working-state projection is active for this chat scope yet.'}
              </pre>
            </div>
            <div className="rounded-2xl border border-[var(--border-default)] bg-[var(--bg-card)] px-3 py-3">
              <p className="text-[10px] uppercase tracking-[0.2em] text-[var(--text-placeholder)]">Profile defaults</p>
              <pre className="mt-2 whitespace-pre-wrap break-words font-mono text-[11px] leading-5 text-[var(--text-secondary)]">
                {profilePreview ?? 'No structured user-profile projection is available for this scope yet.'}
              </pre>
            </div>
          </div>
        </section>

        <section className="glass-card overflow-hidden">
          <div className="border-b border-[var(--border-default)] px-4 py-3">
            <div className="flex items-center gap-2">
              <BrainCircuit className="h-4 w-4 text-[var(--accent-primary)]" />
              <div>
                <p className="text-[10px] font-semibold uppercase tracking-[0.24em] text-[var(--text-placeholder)]">
                  Hippocampus
                </p>
                <h3 className="text-sm font-semibold text-[var(--text-primary)]">Memory Surface</h3>
              </div>
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
              <Layers3 className="h-4 w-4 text-[var(--accent-primary)]" />
              <div>
                <p className="text-[10px] font-semibold uppercase tracking-[0.24em] text-[var(--text-placeholder)]">
                  Neocortex
                </p>
                <h3 className="text-sm font-semibold text-[var(--text-primary)]">Procedural Surface</h3>
              </div>
            </div>
          </div>
          <div className="space-y-3 px-4 py-4">
            {topEffectiveSkills.length > 0 ? (
              <>
                <div className="flex flex-wrap gap-1.5">
                  {topEffectiveSkills.map((skill) => (
                    <TinyBadge key={`${skill.name}-${skill.origin}`}>{skill.name}</TinyBadge>
                  ))}
                </div>
                <div className="grid grid-cols-2 gap-3">
                  <div className="rounded-2xl border border-[var(--border-default)] bg-[var(--bg-card)] px-3 py-3">
                    <p className="text-[10px] uppercase tracking-[0.2em] text-[var(--text-placeholder)]">Recipes</p>
                    <p className="mt-1 text-lg font-semibold text-[var(--text-primary)]">
                      {projections?.run_recipes.length ?? 0}
                    </p>
                  </div>
                  <div className="rounded-2xl border border-[var(--border-default)] bg-[var(--bg-card)] px-3 py-3">
                    <p className="text-[10px] uppercase tracking-[0.2em] text-[var(--text-placeholder)]">Clusters</p>
                    <p className="mt-1 text-lg font-semibold text-[var(--text-primary)]">
                      {(projections?.recipe_clusters.length ?? 0) + (projections?.precedent_clusters.length ?? 0)}
                    </p>
                  </div>
                </div>
              </>
            ) : (
              <p className="text-xs text-[var(--text-muted)]">
                No effective procedural surface has been promoted yet.
              </p>
            )}
          </div>
        </section>

        <section className="glass-card overflow-hidden">
          <div className="border-b border-[var(--border-default)] px-4 py-3">
            <div className="flex items-center gap-2">
              <Gauge className="h-4 w-4 text-[var(--accent-primary)]" />
              <div>
                <p className="text-[10px] font-semibold uppercase tracking-[0.24em] text-[var(--text-placeholder)]">
                  Praefrontalis
                </p>
                <h3 className="text-sm font-semibold text-[var(--text-primary)]">Context Budget</h3>
              </div>
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
                    label: 'Nearby',
                    share: nearbyShare,
                    meta: `${budget.nearby_max_entries} echo lanes`,
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
              <div>
                <p className="text-[10px] font-semibold uppercase tracking-[0.24em] text-[var(--text-placeholder)]">
                  Corpus Sessionis
                </p>
                <h3 className="text-sm font-semibold text-[var(--text-primary)]">Session Lens</h3>
              </div>
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
