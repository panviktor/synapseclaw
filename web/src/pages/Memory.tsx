import { Fragment, useState, useEffect, useMemo, useCallback, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { useNavigate, useSearchParams } from 'react-router-dom';
import {
  Activity,
  Archive,
  Brain,
  BrainCircuit,
  Search,
  Plus,
  Trash2,
  X,
  Filter,
  ChevronUp,
  ChevronDown,
  Gauge,
  Layers3,
  BookMarked,
  Sparkles,
  ArrowRight,
  Orbit,
  Target,
  type LucideIcon,
} from 'lucide-react';
import type {
  ContextBudgetResponse,
  LearningMaintenancePlanResponse,
  LearningMaintenanceSnapshotResponse,
  MemoryEntry,
  MemoryProjectionsResponse,
  MemoryStatsResponse,
  ProjectionRef,
  ProceduralClusterReviewResponse,
  RunRecipeReviewDecisionResponse,
  SkillSurfaceEntry,
  SkillReviewDecisionResponse,
} from '@/types/api';
import {
  getMemory,
  storeMemory,
  deleteMemory,
  getMemoryStats,
  getContextBudget,
  getMemoryProjections,
} from '@/lib/api';
import { t } from '@/lib/i18n';

function truncate(text: string, max: number): string {
  if (text.length <= max) return text;
  return text.slice(0, max) + '...';
}

function formatDate(iso: string): string {
  const d = new Date(iso);
  return d.toLocaleString();
}

function formatNumber(value: number): string {
  return new Intl.NumberFormat().format(value);
}

function formatChars(value: number): string {
  return `${formatNumber(value)} chars`;
}

function budgetShare(value: number, total: number): number {
  if (total <= 0 || value <= 0) return 0;
  return Math.max(8, Math.round((value / total) * 100));
}

type SortField = 'key' | 'category' | 'timestamp';
type SortDir = 'asc' | 'desc';
type StudioTab = 'praefrontalis' | 'hippocampus' | 'neocortex' | 'amygdala' | 'archivum';

const CHAMBERS: Array<{
  id: StudioTab;
  latin: string;
  title: string;
  subtitle: string;
  icon: LucideIcon;
}> = [
  {
    id: 'praefrontalis',
    latin: 'Praefrontalis',
    title: 'Cortex Praefrontalis',
    subtitle: 'working state, profile, budget, governance',
    icon: BrainCircuit,
  },
  {
    id: 'hippocampus',
    latin: 'Hippocampus',
    title: 'Memoria Episodica',
    subtitle: 'sessions, precedents, reflections, cluster recall',
    icon: Orbit,
  },
  {
    id: 'neocortex',
    latin: 'Neocortex',
    title: 'Procedural Layer',
    subtitle: 'core memory, skills, recipes, semantic structure',
    icon: Layers3,
  },
  {
    id: 'amygdala',
    latin: 'Amygdala',
    title: 'Stress And Conflict',
    subtitle: 'failures, contradictions, reviews, maintenance',
    icon: Activity,
  },
  {
    id: 'archivum',
    latin: 'Archivum',
    title: 'Raw Archive',
    subtitle: 'search, inspect, add, delete, verify',
    icon: Archive,
  },
];

function formatLabel(value?: string | null): string {
  if (!value) return 'unknown';
  return value.replace(/[_-]+/g, ' ');
}

function summarizeProjectionRef(item: ProjectionRef): string {
  return (
    item.task_family ||
    item.representative_task_family ||
    item.key ||
    item.representative_key ||
    item.kind ||
    'untitled trace'
  );
}

function toneBadge(active: boolean): string {
  return active
    ? 'border-transparent bg-[var(--accent-primary)] text-white shadow-[0_18px_48px_var(--glow-primary)]'
    : 'border-[var(--border-default)] bg-[var(--bg-card)]/80 text-[var(--text-muted)] hover:border-[var(--accent-primary)]/35 hover:text-[var(--text-primary)]';
}

function ChamberButton({
  chamber,
  active,
  onClick,
}: {
  chamber: (typeof CHAMBERS)[number];
  active: boolean;
  onClick: () => void;
}) {
  const Icon = chamber.icon;

  return (
    <button
      onClick={onClick}
      className={`relative overflow-hidden rounded-[26px] border p-4 text-left transition-all duration-300 ${toneBadge(active)}`}
    >
      <div className="absolute inset-x-6 top-0 h-px bg-gradient-to-r from-transparent via-white/55 to-transparent" />
      <div className="flex items-start justify-between gap-3">
        <div>
          <p className="text-[10px] font-semibold uppercase tracking-[0.28em] opacity-75">
            {chamber.latin}
          </p>
          <p className="mt-2 text-sm font-semibold tracking-tight">{chamber.title}</p>
          <p className="mt-1 text-xs leading-5 opacity-80">{chamber.subtitle}</p>
        </div>
        <div className={`rounded-2xl p-2 ${active ? 'bg-white/16' : 'bg-[var(--glow-secondary)]'}`}>
          <Icon className="h-5 w-5" />
        </div>
      </div>
    </button>
  );
}

function AtlasMetric({
  label,
  value,
  caption,
}: {
  label: string;
  value: string;
  caption: string;
}) {
  return (
    <div className="rounded-[26px] border border-[var(--border-default)] bg-[var(--bg-card)]/90 px-4 py-4 shadow-[0_20px_50px_rgba(12,16,24,0.08)] backdrop-blur">
      <p className="text-[10px] font-semibold uppercase tracking-[0.28em] text-[var(--text-placeholder)]">{label}</p>
      <p className="mt-2 text-2xl font-semibold tracking-tight text-[var(--text-primary)]">{value}</p>
      <p className="mt-1 text-xs leading-5 text-[var(--text-muted)]">{caption}</p>
    </div>
  );
}

function PanelShell({
  eyebrow,
  title,
  icon: Icon,
  children,
  actions,
  className = '',
}: {
  eyebrow: string;
  title: string;
  icon: LucideIcon;
  children: ReactNode;
  actions?: ReactNode;
  className?: string;
}) {
  return (
    <section className={`relative overflow-hidden rounded-[30px] border border-[var(--border-default)] bg-[linear-gradient(180deg,rgba(255,255,255,0.94),rgba(255,248,241,0.92))] shadow-[0_30px_80px_rgba(12,16,24,0.08)] ${className}`}>
      <div className="absolute inset-x-10 top-0 h-px bg-gradient-to-r from-transparent via-[var(--accent-primary)]/60 to-transparent" />
      <div className="flex items-start justify-between gap-3 border-b border-[var(--border-default)]/80 px-5 py-4">
        <div className="flex items-start gap-3">
          <div className="rounded-2xl bg-[var(--glow-primary)] p-2.5 text-[var(--accent-primary)]">
            <Icon className="h-5 w-5" />
          </div>
          <div>
            <p className="text-[10px] font-semibold uppercase tracking-[0.28em] text-[var(--text-placeholder)]">
              {eyebrow}
            </p>
            <h2 className="mt-1 text-lg font-semibold tracking-tight text-[var(--text-primary)]">
              {title}
            </h2>
          </div>
        </div>
        {actions}
      </div>
      <div className="px-5 py-5">{children}</div>
    </section>
  );
}

function ProjectionText({
  text,
  empty,
  compact = false,
}: {
  text?: string | null;
  empty: string;
  compact?: boolean;
}) {
  if (!text?.trim()) {
    return <p className="text-sm leading-6 text-[var(--text-muted)]">{empty}</p>;
  }

  return (
    <pre
      className={`overflow-x-auto whitespace-pre-wrap break-words rounded-[24px] border border-[var(--border-default)] bg-[var(--bg-card)]/75 px-4 py-4 font-mono text-[12px] leading-6 text-[var(--text-secondary)] ${compact ? 'max-h-64 overflow-y-auto' : 'min-h-[9rem]'}`}
    >
      {text}
    </pre>
  );
}

function TinyBadge({ children }: { children: ReactNode }) {
  return (
    <span className="inline-flex items-center rounded-full border border-[var(--border-default)] bg-[var(--glow-secondary)] px-2.5 py-1 text-[10px] font-semibold uppercase tracking-wide text-[var(--text-muted)]">
      {children}
    </span>
  );
}

function formatReason(reason: string): string {
  return reason.replace(/[_-]+/g, ' ');
}

function DecisionDeck({
  eyebrow,
  title,
  icon,
  items,
  empty,
}: {
  eyebrow: string;
  title: string;
  icon: LucideIcon;
  items: Array<{
    key: string;
    title: string;
    subtitle?: string | null;
    action: string;
    reason: string;
    badges?: string[];
    body?: string | null;
  }>;
  empty: string;
}) {
  return (
    <PanelShell eyebrow={eyebrow} title={title} icon={icon}>
      {items.length === 0 ? (
        <p className="text-sm leading-6 text-[var(--text-muted)]">{empty}</p>
      ) : (
        <div className="grid gap-4 lg:grid-cols-2">
          {items.map((item) => (
            <article
              key={item.key}
              className="rounded-[24px] border border-[var(--border-default)] bg-[var(--bg-card)]/80 p-4"
            >
              <div className="flex flex-wrap items-start justify-between gap-3">
                <div>
                  <h3 className="text-sm font-semibold text-[var(--text-primary)]">{item.title}</h3>
                  {item.subtitle && (
                    <p className="mt-1 text-xs text-[var(--text-muted)]">{item.subtitle}</p>
                  )}
                </div>
                <TinyBadge>{formatReason(item.action)}</TinyBadge>
              </div>
              <p className="mt-3 text-xs leading-6 text-[var(--text-muted)]">
                {formatReason(item.reason)}
              </p>
              {item.badges?.length ? (
                <div className="mt-3 flex flex-wrap gap-1.5">
                  {item.badges.map((badge) => (
                    <TinyBadge key={badge}>{badge}</TinyBadge>
                  ))}
                </div>
              ) : null}
              {item.body ? (
                <pre className="mt-4 max-h-56 overflow-y-auto whitespace-pre-wrap break-words rounded-[20px] bg-[var(--bg-primary)]/65 px-3 py-3 font-mono text-[11px] leading-6 text-[var(--text-secondary)]">
                  {item.body}
                </pre>
              ) : null}
            </article>
          ))}
        </div>
      )}
    </PanelShell>
  );
}

function buildSkillDecisionItems(items: SkillReviewDecisionResponse[]) {
  return items.map((item) => ({
    key: `${item.skill_id}-${item.action}-${item.reason}`,
    title: item.skill_name,
    subtitle: `target status: ${formatLabel(item.target_status)}`,
    action: item.action,
    reason: item.reason,
    badges: item.lineage_task_families.map(formatLabel),
  }));
}

function buildRecipeDecisionItems(items: RunRecipeReviewDecisionResponse[]) {
  return items.map((item) => ({
    key: `${item.canonical_recipe.task_family}-${item.reason}`,
    title: formatLabel(item.canonical_recipe.task_family),
    subtitle: `${item.canonical_recipe.success_count} successes`,
    action: item.promotion_blocked ? 'promotion blocked' : 'recipe review',
    reason: item.promotion_block_reason ?? item.reason,
    badges: [
      ...item.cluster_task_families.map(formatLabel),
      ...item.canonical_recipe.lineage_task_families.map(formatLabel),
    ].slice(0, 8),
    body: [
      item.canonical_recipe.summary,
      item.canonical_recipe.tool_pattern.length
        ? `tools: ${item.canonical_recipe.tool_pattern.join(' -> ')}`
        : null,
      item.removed_task_families.length
        ? `removed: ${item.removed_task_families.map(formatLabel).join(', ')}`
        : null,
    ]
      .filter(Boolean)
      .join('\n'),
  }));
}

function buildClusterReviewItems(items: ProceduralClusterReviewResponse[]) {
  return items.map((item) => ({
    key: `${item.kind}-${item.representative_key}-${item.action}`,
    title: item.representative_key,
    subtitle: `${item.member_count} members`,
    action: item.action,
    reason: item.reason,
    badges: [formatLabel(item.kind)],
  }));
}

function ProjectionDeck({
  title,
  eyebrow,
  icon,
  items,
  empty,
}: {
  title: string;
  eyebrow: string;
  icon: LucideIcon;
  items: ProjectionRef[];
  empty: string;
}) {
  return (
    <PanelShell eyebrow={eyebrow} title={title} icon={icon}>
      {items.length === 0 ? (
        <p className="text-sm leading-6 text-[var(--text-muted)]">{empty}</p>
      ) : (
        <div className="grid gap-4 lg:grid-cols-2">
          {items.map((item, index) => (
            <article
              key={`${item.key ?? item.representative_key ?? item.task_family ?? item.representative_task_family ?? item.kind ?? 'projection'}-${index}`}
              className="rounded-[24px] border border-[var(--border-default)] bg-[var(--bg-card)]/80 p-4"
            >
              <div className="flex flex-wrap items-start justify-between gap-2">
                <div>
                  <h3 className="text-sm font-semibold text-[var(--text-primary)]">
                    {formatLabel(summarizeProjectionRef(item))}
                  </h3>
                  {(item.key || item.representative_key || item.kind) && (
                    <p className="mt-1 text-xs text-[var(--text-muted)]">
                      {[item.key, item.representative_key, item.kind].filter(Boolean).join(' · ')}
                    </p>
                  )}
                </div>
                <div className="flex flex-wrap gap-1.5">
                  {item.member_count != null && <TinyBadge>{item.member_count} nodes</TinyBadge>}
                  {(item.task_family || item.representative_task_family) && (
                    <TinyBadge>{formatLabel(item.task_family ?? item.representative_task_family)}</TinyBadge>
                  )}
                </div>
              </div>
              {item.lineage_task_families?.length ? (
                <div className="mt-3 flex flex-wrap gap-1.5">
                  {item.lineage_task_families.slice(0, 4).map((lineage) => (
                    <TinyBadge key={lineage}>{formatLabel(lineage)}</TinyBadge>
                  ))}
                </div>
              ) : null}
              {item.projection ? (
                <pre className="mt-4 max-h-56 overflow-y-auto whitespace-pre-wrap break-words rounded-[20px] bg-[var(--bg-primary)]/65 px-3 py-3 font-mono text-[11px] leading-6 text-[var(--text-secondary)]">
                  {item.projection}
                </pre>
              ) : (
                <p className="mt-4 text-sm leading-6 text-[var(--text-muted)]">
                  No projection body surfaced for this trace yet.
                </p>
              )}
            </article>
          ))}
        </div>
      )}
    </PanelShell>
  );
}

function SkillDeck({
  title,
  eyebrow,
  items,
  empty,
}: {
  title: string;
  eyebrow: string;
  items: SkillSurfaceEntry[];
  empty: string;
}) {
  return (
    <PanelShell eyebrow={eyebrow} title={title} icon={Sparkles}>
      {items.length === 0 ? (
        <p className="text-sm leading-6 text-[var(--text-muted)]">{empty}</p>
      ) : (
        <div className="grid gap-4 lg:grid-cols-2">
          {items.map((skill) => (
            <article
              key={`${skill.name}-${skill.origin}-${skill.status}-${skill.source}`}
              className="rounded-[24px] border border-[var(--border-default)] bg-[var(--bg-card)]/80 p-4"
            >
              <div className="flex flex-wrap items-start justify-between gap-3">
                <div>
                  <h3 className="text-sm font-semibold text-[var(--text-primary)]">{skill.name}</h3>
                  <p className="mt-1 text-xs text-[var(--text-muted)]">
                    {formatLabel(skill.origin)} · {formatLabel(skill.status)} · {formatLabel(skill.source)}
                  </p>
                </div>
                <div className="flex flex-wrap gap-1.5">
                  {skill.effective && <TinyBadge>effective</TinyBadge>}
                  <TinyBadge>p{skill.priority}</TinyBadge>
                </div>
              </div>
              {skill.shadowed_by && (
                <p className="mt-3 text-xs leading-5 text-[var(--text-muted)]">
                  Shadowed by <span className="font-medium text-[var(--text-primary)]">{skill.shadowed_by}</span>
                </p>
              )}
              {skill.projection ? (
                <pre className="mt-4 max-h-56 overflow-y-auto whitespace-pre-wrap break-words rounded-[20px] bg-[var(--bg-primary)]/65 px-3 py-3 font-mono text-[11px] leading-6 text-[var(--text-secondary)]">
                  {skill.projection}
                </pre>
              ) : (
                <p className="mt-4 text-sm leading-6 text-[var(--text-muted)]">
                  This skill is surfaced without a detailed projection body.
                </p>
              )}
            </article>
          ))}
        </div>
      )}
    </PanelShell>
  );
}

function ContradictionDeck({
  items,
}: {
  items: MemoryProjectionsResponse['procedural_contradictions'];
}) {
  return (
    <PanelShell eyebrow="Procedural Contradictions" title="Conflict Vectors" icon={Target}>
      {items.length === 0 ? (
        <p className="text-sm leading-6 text-[var(--text-muted)]">
          No active recipe-versus-failure contradiction was surfaced in the current window.
        </p>
      ) : (
        <div className="grid gap-4 lg:grid-cols-2">
          {items.map((item, index) => (
            <article
              key={`${item.recipe_task_family}-${item.failure_representative_key}-${index}`}
              className="rounded-[24px] border border-[rgba(217,90,30,0.18)] bg-[linear-gradient(180deg,rgba(255,247,240,0.96),rgba(255,250,245,0.9))] p-4"
            >
              <div className="flex flex-wrap items-start justify-between gap-2">
                <div>
                  <h3 className="text-sm font-semibold text-[var(--text-primary)]">
                    {formatLabel(item.recipe_task_family)}
                  </h3>
                  <p className="mt-1 text-xs text-[var(--text-muted)]">
                    Failure anchor: {item.failure_representative_key}
                  </p>
                </div>
                <TinyBadge>{Math.round(item.overlap * 100)}% overlap</TinyBadge>
              </div>
              <div className="mt-3 flex flex-wrap gap-1.5">
                <TinyBadge>{item.recipe_cluster_size} recipe nodes</TinyBadge>
                <TinyBadge>{item.failure_cluster_size} failure nodes</TinyBadge>
                {item.failed_tools.slice(0, 3).map((tool) => (
                  <TinyBadge key={tool}>{tool}</TinyBadge>
                ))}
              </div>
              {item.recipe_lineage_task_families.length > 0 && (
                <p className="mt-3 text-xs leading-6 text-[var(--text-muted)]">
                  Lineage: {item.recipe_lineage_task_families.map(formatLabel).join(', ')}
                </p>
              )}
            </article>
          ))}
        </div>
      )}
    </PanelShell>
  );
}

function MaintenanceMatrix({
  snapshot,
  plan,
}: {
  snapshot: LearningMaintenanceSnapshotResponse | null | undefined;
  plan: LearningMaintenancePlanResponse | null | undefined;
}) {
  return (
    <PanelShell eyebrow="Autonomic Layer" title="Maintenance Matrix" icon={Activity}>
      {!snapshot || !plan ? (
        <p className="text-sm leading-6 text-[var(--text-muted)]">
          Structured maintenance state is not available for this scope yet.
        </p>
      ) : (
        <div className="space-y-5">
          <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-4">
            {[
              {
                label: 'Recipe pressure',
                value: `${snapshot.recent_run_recipe_count}/${snapshot.run_recipe_cluster_count}`,
                caption: 'recent recipes / clusters',
              },
              {
                label: 'Precedent pressure',
                value: `${snapshot.precedent_compact_candidate_count}`,
                caption: 'compact candidates',
              },
              {
                label: 'Failure pressure',
                value: `${snapshot.failure_pattern_blocking_count}`,
                caption: 'blocking clusters',
              },
              {
                label: 'Skill review',
                value: `${snapshot.recent_skill_count}/${snapshot.candidate_skill_count}`,
                caption: 'active / candidate skills',
              },
            ].map((item) => (
              <div
                key={item.label}
                className="rounded-[22px] border border-[var(--border-default)] bg-[var(--bg-card)]/80 px-4 py-3"
              >
                <p className="text-[10px] uppercase tracking-[0.24em] text-[var(--text-placeholder)]">
                  {item.label}
                </p>
                <p className="mt-2 text-xl font-semibold text-[var(--text-primary)]">{item.value}</p>
                <p className="mt-1 text-xs leading-5 text-[var(--text-muted)]">{item.caption}</p>
              </div>
            ))}
          </div>

          <div className="grid gap-4 xl:grid-cols-[1.2fr_0.8fr]">
            <div className="rounded-[24px] border border-[var(--border-default)] bg-[var(--bg-card)]/80 p-4">
              <p className="text-[10px] font-semibold uppercase tracking-[0.26em] text-[var(--text-placeholder)]">
                Planned actions
              </p>
              <div className="mt-3 flex flex-wrap gap-2">
                {[
                  ['importance decay', plan.run_importance_decay],
                  ['garbage collect', plan.run_gc],
                  ['recipe review', plan.run_run_recipe_review],
                  ['precedent compaction', plan.run_precedent_compaction],
                  ['failure compaction', plan.run_failure_pattern_compaction],
                  ['skill review', plan.run_skill_review],
                  ['prompt optimization', plan.run_prompt_optimization],
                ].map(([label, active]) => (
                  <span
                    key={String(label)}
                    className={`inline-flex items-center rounded-full border px-2.5 py-1 text-[10px] font-semibold uppercase tracking-wide ${
                      active
                        ? 'border-transparent bg-[var(--accent-primary)] text-white'
                        : 'border-[var(--border-default)] bg-[var(--glow-secondary)] text-[var(--text-muted)]'
                    }`}
                  >
                    {String(label)}
                  </span>
                ))}
              </div>
            </div>

            <div className="rounded-[24px] border border-[var(--border-default)] bg-[var(--bg-card)]/80 p-4">
              <p className="text-[10px] font-semibold uppercase tracking-[0.26em] text-[var(--text-placeholder)]">
                Reasons and cadence
              </p>
              <div className="mt-3 flex flex-wrap gap-1.5">
                {plan.reasons.length > 0 ? (
                  plan.reasons.map((reason) => (
                    <TinyBadge key={reason}>{formatLabel(reason)}</TinyBadge>
                  ))
                ) : (
                  <TinyBadge>no active reason</TinyBadge>
                )}
              </div>
              <div className="mt-4 grid gap-3 sm:grid-cols-2">
                <div className="rounded-[18px] bg-[var(--bg-primary)]/65 px-3 py-3">
                  <p className="text-[10px] uppercase tracking-[0.2em] text-[var(--text-placeholder)]">
                    Skipped cycles
                  </p>
                  <p className="mt-1 text-sm font-semibold text-[var(--text-primary)]">
                    {snapshot.skipped_cycles_since_maintenance}
                  </p>
                </div>
                <div className="rounded-[18px] bg-[var(--bg-primary)]/65 px-3 py-3">
                  <p className="text-[10px] uppercase tracking-[0.2em] text-[var(--text-placeholder)]">
                    Prompt rewrite due
                  </p>
                  <p className="mt-1 text-sm font-semibold text-[var(--text-primary)]">
                    {snapshot.prompt_optimization_due ? 'yes' : 'no'}
                  </p>
                </div>
              </div>
            </div>
          </div>
        </div>
      )}
    </PanelShell>
  );
}

export default function Memory() {
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const selectedAgent = searchParams.get('agent');
  const remoteScope = Boolean(selectedAgent);

  const [entries, setEntries] = useState<MemoryEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [surfaceLoading, setSurfaceLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [stats, setStats] = useState<MemoryStatsResponse | null>(null);
  const [budget, setBudget] = useState<ContextBudgetResponse | null>(null);
  const [projections, setProjections] = useState<MemoryProjectionsResponse | null>(null);
  const [search, setSearch] = useState('');
  const [categoryFilter, setCategoryFilter] = useState('');
  const [showForm, setShowForm] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [sortField, setSortField] = useState<SortField>('timestamp');
  const [sortDir, setSortDir] = useState<SortDir>('desc');
  const [activeTab, setActiveTab] = useState<StudioTab>('praefrontalis');

  const [formKey, setFormKey] = useState('');
  const [formContent, setFormContent] = useState('');
  const [formCategory, setFormCategory] = useState('');
  const [formError, setFormError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const agentLabel = stats?.agent_id ?? selectedAgent ?? 'Local Runtime';
  const topCategories = stats?.by_category.slice(0, 5) ?? [];
  const heroChamber = CHAMBERS.find((chamber) => chamber.id === activeTab) ?? CHAMBERS[0]!;
  const ActiveChamberIcon = heroChamber.icon;

  const fetchEntries = useCallback((q?: string, cat?: string) => {
    if (remoteScope) {
      setEntries([]);
      setLoading(false);
      return;
    }
    setLoading(true);
    getMemory(q || undefined, cat || undefined)
      .then(setEntries)
      .catch((err) => setError(err.message))
      .finally(() => setLoading(false));
  }, [remoteScope]);

  const fetchSurface = useCallback(() => {
    setSurfaceLoading(true);
    Promise.all([
      getMemoryStats(selectedAgent),
      getContextBudget(selectedAgent),
      getMemoryProjections(selectedAgent, 8),
    ])
      .then(([memoryStats, contextBudget, memoryProjections]) => {
        setStats(memoryStats);
        setBudget(contextBudget);
        setProjections(memoryProjections);
      })
      .catch((err) => setError((prev) => prev ?? err.message))
      .finally(() => setSurfaceLoading(false));
  }, [selectedAgent]);

  useEffect(() => {
    fetchSurface();
  }, [fetchSurface]);

  useEffect(() => {
    fetchEntries();
  }, [fetchEntries]);

  useEffect(() => {
    if (!showForm) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        setShowForm(false);
        setFormError(null);
      }
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [showForm]);

  const handleSearch = () => {
    fetchEntries(search, categoryFilter);
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter') handleSearch();
  };

  const categories = Array.from(new Set(entries.map((e) => e.category))).sort();
  const hasScores = entries.some((e) => e.score !== null && e.score !== undefined);
  const colCount = hasScores ? 6 : 5;
  const totalClusterCount =
    (projections?.recipe_clusters.length ?? 0) +
    (projections?.precedent_clusters.length ?? 0) +
    (projections?.failure_pattern_clusters.length ?? 0);
  const lineageFamilies = useMemo(
    () =>
      Array.from(
        new Set(
          [
            ...(projections?.recipe_clusters.flatMap((item) => item.lineage_task_families ?? []) ?? []),
            ...(projections?.run_recipes.flatMap((item) => item.lineage_task_families ?? []) ?? []),
          ].filter(Boolean),
        ),
      ),
    [projections],
  );

  const sortedEntries = useMemo(() => {
    const sorted = [...entries].sort((a, b) => {
      if (sortField === 'timestamp') {
        return new Date(a.timestamp).getTime() - new Date(b.timestamp).getTime();
      }
      return (a[sortField] ?? '').localeCompare(b[sortField] ?? '');
    });
    return sortDir === 'desc' ? sorted.reverse() : sorted;
  }, [entries, sortField, sortDir]);

  const toggleSort = (field: SortField) => {
    if (sortField === field) {
      setSortDir((d) => (d === 'asc' ? 'desc' : 'asc'));
    } else {
      setSortField(field);
      setSortDir(field === 'timestamp' ? 'desc' : 'asc');
    }
  };

  const handleAdd = async () => {
    if (!formKey.trim() || !formContent.trim()) {
      setFormError(t('memory.validation_error'));
      return;
    }
    setSubmitting(true);
    setFormError(null);
    try {
      await storeMemory(
        formKey.trim(),
        formContent.trim(),
        formCategory.trim() || undefined,
      );
      fetchEntries(search, categoryFilter);
      fetchSurface();
      setShowForm(false);
      setFormKey('');
      setFormContent('');
      setFormCategory('');
    } catch (err: unknown) {
      setFormError(err instanceof Error ? err.message : t('memory.store_error'));
    } finally {
      setSubmitting(false);
    }
  };

  const handleDelete = async (key: string) => {
    try {
      await deleteMemory(key);
      setEntries((prev) => prev.filter((e) => e.key !== key));
      fetchSurface();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : t('memory.delete_error'));
    } finally {
      setConfirmDelete(null);
    }
  };

  const SortHeader = ({ field, label }: { field: SortField; label: string }) => (
    <th
      className="text-left cursor-pointer select-none hover:text-[var(--text-primary)] transition-colors"
      onClick={() => toggleSort(field)}
    >
      <div className="flex items-center gap-1">
        {label}
        {sortField === field ? (
          sortDir === 'asc' ? (
            <ChevronUp className="h-3 w-3 text-[var(--accent-primary)]" />
          ) : (
            <ChevronDown className="h-3 w-3 text-[var(--accent-primary)]" />
          )
        ) : (
          <ChevronDown className="h-3 w-3 text-[var(--text-placeholder)] opacity-0 group-hover:opacity-50" />
        )}
      </div>
    </th>
  );

  return (
    <div className="space-y-6 p-6 animate-fade-in">
      <div className="relative overflow-hidden rounded-[36px] border border-[var(--border-default)] bg-[radial-gradient(circle_at_top_left,rgba(217,90,30,0.24),transparent_38%),radial-gradient(circle_at_85%_18%,rgba(255,196,128,0.2),transparent_26%),linear-gradient(135deg,rgba(255,248,241,0.98),rgba(255,255,255,0.92))] px-6 py-7 shadow-[0_35px_120px_rgba(12,16,24,0.12)]">
        <div className="absolute -left-12 top-8 h-36 w-36 rounded-full bg-[var(--glow-primary)] blur-3xl" />
        <div className="absolute right-0 top-0 h-48 w-48 rounded-full bg-[rgba(255,211,168,0.28)] blur-3xl" />
        <div className="absolute inset-x-10 top-0 h-px bg-gradient-to-r from-transparent via-[var(--accent-primary)]/80 to-transparent" />
        <div className="relative flex flex-col gap-6 xl:flex-row xl:items-end xl:justify-between">
          <div>
            <p className="text-[11px] font-semibold uppercase tracking-[0.3em] text-[var(--text-placeholder)]">
              Atlas Memoriae
            </p>
            <div className="mt-3 flex flex-wrap items-center gap-2">
              <h1 className="text-4xl font-semibold tracking-tight text-[var(--text-primary)]">
                Memory Atlas For {agentLabel}
              </h1>
              <span className={`rounded-full px-2.5 py-1 text-[10px] font-semibold uppercase tracking-wide ${
                remoteScope
                  ? 'bg-[var(--status-info)]/10 text-[var(--status-info)]'
                  : 'bg-[var(--status-success)]/12 text-[var(--status-success)]'
              }`}>
                {remoteScope ? 'remote scope' : 'local scope'}
              </span>
            </div>
            <p className="mt-3 max-w-3xl text-sm leading-7 text-[var(--text-muted)]">
              A dramatic operator map for the new memory system: working state, episodic recall,
              procedural skill lines, contradictions, maintenance pressure, and the raw archive,
              all surfaced in one place instead of a dead table.
            </p>
            <div className="mt-4 flex flex-wrap gap-2">
              {['Praefrontalis', 'Hippocampus', 'Neocortex', 'Amygdala', 'Archivum'].map((label) => (
                <TinyBadge key={label}>{label}</TinyBadge>
              ))}
            </div>
          </div>

          <div className="w-full max-w-md rounded-[28px] border border-[var(--border-default)] bg-white/75 p-5 backdrop-blur xl:w-[28rem]">
            <div className="flex items-start justify-between gap-4">
              <div>
                <p className="text-[10px] font-semibold uppercase tracking-[0.28em] text-[var(--text-placeholder)]">
                  Active Chamber
                </p>
                <p className="mt-2 text-lg font-semibold text-[var(--text-primary)]">
                  {heroChamber.title}
                </p>
                <p className="mt-1 text-sm leading-6 text-[var(--text-muted)]">
                  {heroChamber.subtitle}
                </p>
              </div>
              <div className="rounded-2xl bg-[var(--glow-primary)] p-3 text-[var(--accent-primary)]">
                <ActiveChamberIcon className="h-5 w-5" />
              </div>
            </div>
            <div className="mt-5 flex flex-wrap gap-2">
              {remoteScope && (
                <button
                  onClick={() => navigate(`/agents?agent=${encodeURIComponent(selectedAgent ?? '')}`)}
                  className="btn-secondary inline-flex items-center gap-2 px-4 py-2 text-sm"
                >
                  <Orbit className="h-4 w-4" />
                  Back To Workbench
                </button>
              )}
              {!remoteScope && (
                <button
                  onClick={() => setShowForm(true)}
                  className="btn-primary inline-flex items-center gap-2 px-4 py-2 text-sm"
                >
                  <Plus className="h-4 w-4" />
                  {t('memory.add_memory')}
                </button>
              )}
            </div>
          </div>
        </div>
      </div>

      {error && (
        <div className="rounded-xl border border-[#ff446630] bg-[#ff446615] p-3 text-sm text-[#ff6680] animate-fade-in">
          {error}
        </div>
      )}

      <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-5">
        <AtlasMetric
          label="Engrammata"
          value={surfaceLoading ? '…' : formatNumber(stats?.total_entries ?? 0)}
          caption="Total entries under the current scope."
        />
        <AtlasMetric
          label="Artes"
          value={surfaceLoading ? '…' : formatNumber(projections?.effective_skills.length ?? stats?.skills ?? 0)}
          caption="Effective skills surviving shadowing and review."
        />
        <AtlasMetric
          label="Clusters"
          value={surfaceLoading ? '…' : formatNumber(totalClusterCount)}
          caption="Recipe, precedent, and failure cluster surface."
        />
        <AtlasMetric
          label="Contradictions"
          value={surfaceLoading ? '…' : formatNumber(projections?.procedural_contradictions.length ?? 0)}
          caption="Recipe branches currently colliding with failure memory."
        />
        <AtlasMetric
          label="Continuatio"
          value={surfaceLoading ? '…' : (budget?.continuation_policy ?? 'n/a')}
          caption="Continuation policy currently shaping the prompt envelope."
        />
      </div>

      <div className="grid gap-3 xl:grid-cols-5">
        {CHAMBERS.map((chamber) => (
          <ChamberButton
            key={chamber.id}
            chamber={chamber}
            active={activeTab === chamber.id}
            onClick={() => setActiveTab(chamber.id)}
          />
        ))}
      </div>

      {activeTab === 'praefrontalis' && (
        <div className="grid gap-6 xl:grid-cols-[1.2fr_0.8fr]">
          <div className="space-y-6">
            <PanelShell eyebrow="Executive Memory" title="Working State" icon={Brain}>
              <ProjectionText
                text={projections?.working_state?.projection}
                empty="No working-state projection has been surfaced yet."
              />
            </PanelShell>

            <PanelShell eyebrow="Learning Digest" title="Recent System Pulse" icon={Sparkles}>
              <ProjectionText
                text={projections?.learning_digest}
                empty="No learning digest has been generated yet for this scope."
              />
            </PanelShell>

            <PanelShell eyebrow="Governance" title="Conflict Policy" icon={BookMarked}>
              <ProjectionText
                text={projections?.skill_conflict_policy}
                empty="Skill conflict policy has not been exposed yet."
              />
            </PanelShell>
          </div>

          <div className="space-y-6">
            <PanelShell eyebrow="Profile" title="Current User Profile" icon={Target}>
              <ProjectionText
                text={projections?.current_user_profile?.projection}
                empty="No profile projection is available for this scope."
                compact
              />
            </PanelShell>

            <PanelShell eyebrow="Context Envelope" title="Budget Anatomy" icon={Gauge}>
              {budget ? (
                <div className="space-y-4">
                  {[
                    {
                      label: 'Recall',
                      share: budgetShare(budget.recall_total_max_chars, budget.enrichment_total_max_chars),
                      meta: `${budget.recall_max_entries} entries · ${formatChars(budget.recall_total_max_chars)}`,
                    },
                    {
                      label: 'Nearby',
                      share: budgetShare(
                        budget.nearby_max_entries * Math.min(160, budget.recall_entry_max_chars),
                        budget.enrichment_total_max_chars,
                      ),
                      meta: `${budget.nearby_max_entries} echo lanes`,
                    },
                    {
                      label: 'Skills',
                      share: budgetShare(budget.skills_total_max_chars, budget.enrichment_total_max_chars),
                      meta: `${budget.skills_max_count} items · ${formatChars(budget.skills_total_max_chars)}`,
                    },
                    {
                      label: 'Entities',
                      share: budgetShare(budget.entities_total_max_chars, budget.enrichment_total_max_chars),
                      meta: `${budget.entities_max_count} items · ${formatChars(budget.entities_total_max_chars)}`,
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

                  <div className="grid gap-3">
                    <div className="rounded-[22px] border border-[var(--border-default)] bg-[var(--bg-card)]/75 px-4 py-3">
                      <p className="text-[10px] uppercase tracking-[0.24em] text-[var(--text-placeholder)]">Continuation</p>
                      <p className="mt-1 text-sm font-semibold text-[var(--text-primary)]">{budget.continuation_policy}</p>
                    </div>
                    <div className="rounded-[22px] border border-[var(--border-default)] bg-[var(--bg-card)]/75 px-4 py-3">
                      <p className="text-[10px] uppercase tracking-[0.24em] text-[var(--text-placeholder)]">Envelope</p>
                      <p className="mt-1 text-sm font-semibold text-[var(--text-primary)]">
                        {formatChars(budget.enrichment_total_max_chars)}
                      </p>
                    </div>
                    <div className="rounded-[22px] border border-[var(--border-default)] bg-[var(--bg-card)]/75 px-4 py-3">
                      <p className="text-[10px] uppercase tracking-[0.24em] text-[var(--text-placeholder)]">Min relevance</p>
                      <p className="mt-1 text-sm font-semibold text-[var(--text-primary)]">
                        {budget.min_relevance_score.toFixed(2)}
                      </p>
                    </div>
                  </div>
                </div>
              ) : (
                <p className="text-sm leading-6 text-[var(--text-muted)]">
                  Context budget is not available for this scope yet.
                </p>
              )}
            </PanelShell>

            <PanelShell eyebrow="Cortical Surface" title="Category And Block Topology" icon={Layers3}>
              <div className="space-y-5">
                <div className="space-y-3">
                  <p className="text-xs font-semibold uppercase tracking-[0.24em] text-[var(--text-placeholder)]">
                    Dominant categories
                  </p>
                  {topCategories.length === 0 ? (
                    <p className="text-sm leading-6 text-[var(--text-muted)]">No category distribution has been surfaced yet.</p>
                  ) : (
                    topCategories.map((category) => {
                      const total = stats?.total_entries ?? 0;
                      const width = total > 0 ? Math.max(8, Math.round((category.count / total) * 100)) : 0;
                      return (
                        <div key={category.category} className="space-y-1.5">
                          <div className="flex items-center justify-between gap-3 text-sm">
                            <span className="capitalize text-[var(--text-secondary)]">{category.category}</span>
                            <span className="text-[var(--text-muted)]">{formatNumber(category.count)}</span>
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

                <div className="space-y-3">
                  <p className="text-xs font-semibold uppercase tracking-[0.24em] text-[var(--text-placeholder)]">
                    Core blocks
                  </p>
                  {stats?.core_blocks.length ? (
                    <div className="grid gap-3">
                      {stats.core_blocks.map((block) => (
                        <div key={block.label} className="rounded-[22px] border border-[var(--border-default)] bg-[var(--bg-card)]/75 px-4 py-3">
                          <div className="flex items-center justify-between gap-3">
                            <span className="text-sm font-medium text-[var(--text-primary)]">
                              {formatLabel(block.label)}
                            </span>
                            <span className="text-xs text-[var(--text-muted)]">{formatChars(block.chars)}</span>
                          </div>
                          <p className="mt-1 text-xs text-[var(--text-muted)]">Updated {formatDate(block.updated_at)}</p>
                        </div>
                      ))}
                    </div>
                  ) : (
                    <p className="text-sm leading-6 text-[var(--text-muted)]">
                      No core blocks have been surfaced for this agent yet.
                    </p>
                  )}
                </div>
              </div>
            </PanelShell>
          </div>
        </div>
      )}

      {activeTab === 'hippocampus' && (
        <div className="grid gap-6">
          <ProjectionDeck
            title="Recent Sessions"
            eyebrow="Hippocampus"
            icon={Orbit}
            items={projections?.recent_sessions ?? []}
            empty="No recent sessions were surfaced in the current memory window."
          />
          <ProjectionDeck
            title="Precedent Traces"
            eyebrow="Episodic Memory"
            icon={Brain}
            items={projections?.recent_precedents ?? []}
            empty="No recent precedents have been materialized yet."
          />
          <ProjectionDeck
            title="Reflection Stream"
            eyebrow="Reflective Memory"
            icon={Sparkles}
            items={projections?.recent_reflections ?? []}
            empty="No recent reflections are available in this scope."
          />
          <ProjectionDeck
            title="Precedent Clusters"
            eyebrow="Cluster Review"
            icon={Layers3}
            items={projections?.precedent_clusters ?? []}
            empty="No precedent clusters were formed in the current review window."
          />
        </div>
      )}

      {activeTab === 'neocortex' && (
        <div className="space-y-6">
          <div className="grid gap-6 xl:grid-cols-[1.15fr_0.85fr]">
            <PanelShell eyebrow="Semantic Memory" title="Core Memory" icon={BookMarked}>
              <ProjectionText
                text={projections?.core_memory}
                empty="Core memory has not been surfaced for this scope yet."
              />
            </PanelShell>

            <PanelShell eyebrow="Effective Surface" title="Skill Constellation" icon={Sparkles}>
              <div className="mb-4 flex flex-wrap gap-1.5">
                {lineageFamilies.slice(0, 8).map((family) => (
                  <TinyBadge key={family}>{formatLabel(family)}</TinyBadge>
                ))}
                {lineageFamilies.length === 0 && <TinyBadge>no lineage surfaced</TinyBadge>}
              </div>
              <ProjectionText
                text={projections?.skill_review}
                empty="Skill review projection is not available yet."
                compact
              />
            </PanelShell>
          </div>

          <DecisionDeck
            eyebrow="Skill Review"
            title="Structured Skill Decisions"
            icon={Sparkles}
            items={buildSkillDecisionItems(projections?.skill_review_decisions ?? [])}
            empty="No structured skill review decisions are currently surfaced."
          />

          <SkillDeck
            title="Effective Skills"
            eyebrow="Neocortex"
            items={projections?.effective_skills ?? []}
            empty="No effective skills are currently active."
          />

          <div className="grid gap-6 xl:grid-cols-2">
            <SkillDeck
              title="Configured And Shadowed Skills"
              eyebrow="Manual And Imported Layer"
              items={projections?.skill_surface ?? []}
              empty="No skill surface is available yet."
            />
            <SkillDeck
              title="Recent Learned Skills"
              eyebrow="Promoted Patterns"
              items={projections?.recent_skills ?? []}
              empty="No recent learned skills are visible in this scope."
            />
          </div>

          <ProjectionDeck
            title="Run Recipes"
            eyebrow="Procedural Memory"
            icon={ArrowRight}
            items={projections?.run_recipes ?? []}
            empty="No run recipes have been surfaced yet."
          />
          <ProjectionDeck
            title="Recipe Clusters"
            eyebrow="Clustered Procedures"
            icon={Layers3}
            items={projections?.recipe_clusters ?? []}
            empty="No recipe clusters are available in the current review window."
          />
        </div>
      )}

      {activeTab === 'amygdala' && (
        <div className="space-y-6">
          <div className="grid gap-6 xl:grid-cols-2">
            <PanelShell eyebrow="Maintenance" title="Learning Maintenance" icon={Activity}>
              <ProjectionText
                text={projections?.learning_maintenance}
                empty="No maintenance plan is currently exposed."
              />
            </PanelShell>
            <PanelShell eyebrow="Review Surface" title="Cluster Review Digest" icon={Layers3}>
              <ProjectionText
                text={projections?.procedural_cluster_review}
                empty="Cluster review digest is not available yet."
              />
            </PanelShell>
          </div>

          <MaintenanceMatrix
            snapshot={projections?.learning_maintenance_snapshot}
            plan={projections?.learning_maintenance_plan}
          />

          <ContradictionDeck items={projections?.procedural_contradictions ?? []} />

          <div className="grid gap-6 xl:grid-cols-2">
            <DecisionDeck
              eyebrow="Recipe Review"
              title="Structured Recipe Decisions"
              icon={ArrowRight}
              items={buildRecipeDecisionItems(projections?.run_recipe_review_decisions ?? [])}
              empty="No structured run recipe review decisions are currently surfaced."
            />
            <DecisionDeck
              eyebrow="Failure Cluster Review"
              title="Cluster Actions"
              icon={Layers3}
              items={buildClusterReviewItems([
                ...(projections?.precedent_cluster_reviews ?? []),
                ...(projections?.failure_pattern_cluster_reviews ?? []),
              ])}
              empty="No structured cluster review actions are currently surfaced."
            />
          </div>

          <div className="grid gap-6 xl:grid-cols-2">
            <PanelShell eyebrow="Contradiction Projection" title="Failure Pressure Map" icon={Target}>
              <ProjectionText
                text={projections?.procedural_contradiction_projection}
                empty="No contradiction projection is currently available."
                compact
              />
            </PanelShell>
            <PanelShell eyebrow="Review Decisions" title="Recipe And Skill Reviews" icon={Sparkles}>
              <div className="space-y-4">
                <ProjectionText
                  text={projections?.skill_review}
                  empty="Skill review decisions have not been projected yet."
                  compact
                />
                <ProjectionText
                  text={projections?.run_recipe_review}
                  empty="Run recipe review decisions have not been projected yet."
                  compact
                />
              </div>
            </PanelShell>
          </div>

          <ProjectionDeck
            title="Failure Patterns"
            eyebrow="Stress Memory"
            icon={Activity}
            items={projections?.recent_failure_patterns ?? []}
            empty="No recent failure patterns are visible in this scope."
          />
          <ProjectionDeck
            title="Failure Pattern Clusters"
            eyebrow="Failure Cluster Surface"
            icon={Layers3}
            items={projections?.failure_pattern_clusters ?? []}
            empty="No failure pattern clusters were surfaced in the recent window."
          />
        </div>
      )}

      {activeTab === 'archivum' && (
        <div className="space-y-6">
          {remoteScope ? (
            <div className="rounded-[30px] border border-[var(--border-default)] bg-[linear-gradient(180deg,rgba(255,255,255,0.96),rgba(255,248,241,0.92))] px-6 py-8 text-center shadow-[0_24px_70px_rgba(12,16,24,0.08)]">
              <Archive className="mx-auto mb-4 h-10 w-10 text-[var(--accent-primary)]" />
              <p className="text-lg font-semibold text-[var(--text-primary)]">Remote archive inspector is not proxied yet</p>
              <p className="mx-auto mt-2 max-w-2xl text-sm text-[var(--text-muted)]">
                This view already uses remote `stats` and `context budget`, but raw entry list, add, and delete are still local-only surfaces.
              </p>
            </div>
          ) : (
            <>
              <PanelShell eyebrow="Archivum" title="Raw Memory Controls" icon={Archive}>
                <p className="text-sm leading-6 text-[var(--text-muted)]">
                  This chamber keeps the operator-grade raw view alive: full search, direct adds,
                  deletes, timestamps, categories, and exact content inspection for every stored
                  memory item.
                </p>
              </PanelShell>

              <div className="flex flex-col gap-3 sm:flex-row">
                <div className="relative flex-1">
                  <Search className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-[var(--text-secondary)]" />
                  <input
                    type="text"
                    value={search}
                    onChange={(e) => setSearch(e.target.value)}
                    onKeyDown={handleKeyDown}
                    placeholder={t('memory.search_placeholder')}
                    className="input-warm w-full pl-10 pr-4 py-2.5 text-sm"
                  />
                </div>
                <div className="relative">
                  <Filter className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-[var(--text-secondary)]" />
                  <select
                    value={categoryFilter}
                    onChange={(e) => setCategoryFilter(e.target.value)}
                    className="input-warm appearance-none cursor-pointer pl-10 pr-8 py-2.5 text-sm"
                  >
                    <option value="">{t('memory.all_categories')}</option>
                    {categories.map((cat) => (
                      <option key={cat} value={cat}>
                        {cat}
                      </option>
                    ))}
                  </select>
                </div>
                <button
                  onClick={handleSearch}
                  className="btn-primary px-4 py-2.5 text-sm"
                >
                  {t('memory.search_button')}
                </button>
              </div>

              {loading ? (
                <div className="flex h-32 items-center justify-center">
                  <div className="h-8 w-8 rounded-full border-2 border-[var(--glow-primary)] border-t-[var(--accent-primary)] animate-spin" />
                </div>
              ) : entries.length === 0 ? (
                <div className="rounded-[30px] border border-[var(--border-default)] bg-[linear-gradient(180deg,rgba(255,255,255,0.96),rgba(255,248,241,0.92))] p-8 text-center shadow-[0_24px_70px_rgba(12,16,24,0.08)]">
                  <Brain className="mx-auto mb-3 h-10 w-10 text-[var(--bg-secondary)]" />
                  <p className="text-[var(--text-secondary)]">{t('memory.empty')}</p>
                </div>
              ) : (
                <div className="overflow-x-auto rounded-[30px] border border-[var(--border-default)] bg-[linear-gradient(180deg,rgba(255,255,255,0.96),rgba(255,248,241,0.92))] shadow-[0_24px_70px_rgba(12,16,24,0.08)]">
                  <table className="table-warm">
                    <thead>
                      <tr className="group">
                        <SortHeader field="key" label={t('memory.key')} />
                        <th className="text-left">{t('memory.content')}</th>
                        <SortHeader field="category" label={t('memory.category')} />
                        <SortHeader field="timestamp" label={t('memory.timestamp')} />
                        {hasScores && <th className="text-left">Score</th>}
                        <th className="text-right">{t('common.actions')}</th>
                      </tr>
                    </thead>
                    <tbody>
                      {sortedEntries.map((entry) => (
                        <Fragment key={entry.id}>
                          <tr
                            className="cursor-pointer transition-colors hover:bg-[var(--bg-secondary)]/50"
                            onClick={() => setExpandedId(expandedId === entry.id ? null : entry.id)}
                          >
                            <td className="px-4 py-3 font-mono text-xs font-medium text-[var(--text-primary)]">
                              {entry.key}
                            </td>
                            <td className="max-w-[300px] px-4 py-3 text-sm text-[var(--text-muted)]">
                              <span title={entry.content}>
                                {truncate(entry.content, 80)}
                              </span>
                            </td>
                            <td className="px-4 py-3">
                              <span className="inline-flex items-center rounded-full border border-[var(--bg-secondary)] px-2.5 py-0.5 text-[10px] font-semibold capitalize text-[var(--text-muted)]" style={{ background: 'var(--glow-primary)' }}>
                                {entry.category || 'uncategorized'}
                              </span>
                            </td>
                            <td className="whitespace-nowrap px-4 py-3 text-xs text-[var(--text-secondary)]">
                              {formatDate(entry.timestamp)}
                            </td>
                            {hasScores && (
                              <td className="px-4 py-3 font-mono text-xs text-[var(--text-muted)]">
                                {entry.score != null ? `${(entry.score * 100).toFixed(0)}%` : '—'}
                              </td>
                            )}
                            <td className="px-4 py-3 text-right" onClick={(e) => e.stopPropagation()}>
                              {confirmDelete === entry.key ? (
                                <div className="flex items-center justify-end gap-2 animate-fade-in">
                                  <span className="text-xs text-[#ff4466]">{t('memory.delete_confirm')}</span>
                                  <button
                                    onClick={() => handleDelete(entry.key)}
                                    className="text-xs font-medium text-[#ff4466] hover:text-[#ff6680]"
                                  >
                                    {t('memory.yes')}
                                  </button>
                                  <button
                                    onClick={() => setConfirmDelete(null)}
                                    className="text-xs font-medium text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
                                  >
                                    {t('memory.no')}
                                  </button>
                                </div>
                              ) : (
                                <button
                                  onClick={() => setConfirmDelete(entry.key)}
                                  className="text-[var(--text-secondary)] transition-all duration-300 hover:text-[#ff4466]"
                                >
                                  <Trash2 className="h-4 w-4" />
                                </button>
                              )}
                            </td>
                          </tr>
                          {expandedId === entry.id && (
                            <tr className="animate-fade-in">
                              <td colSpan={colCount} className="bg-[var(--bg-primary)] px-4 py-3">
                                <pre className="max-h-64 overflow-y-auto whitespace-pre-wrap break-words font-mono text-sm leading-relaxed text-[var(--text-primary)]">
                                  {entry.content}
                                </pre>
                              </td>
                            </tr>
                          )}
                        </Fragment>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
            </>
          )}
        </div>
      )}

      {showForm && !remoteScope && createPortal(
        <div className="fixed inset-0 z-[9999] flex items-center justify-center md:pl-60">
          <div className="absolute inset-0 bg-black/60 backdrop-blur-sm" onClick={() => { setShowForm(false); setFormError(null); }} />
          <div className="relative mx-4 w-full max-w-md overflow-hidden rounded-[30px] border border-[var(--border-default)] bg-[linear-gradient(180deg,rgba(255,255,255,0.98),rgba(255,248,241,0.94))] p-6 shadow-[0_35px_100px_rgba(12,16,24,0.18)] animate-fade-in-scale">
            <div className="mb-4 flex items-center justify-between">
              <h3 className="text-lg font-semibold text-[var(--text-primary)]">{t('memory.add_modal_title')}</h3>
              <button
                onClick={() => {
                  setShowForm(false);
                  setFormError(null);
                }}
                className="text-[var(--text-secondary)] transition-colors duration-300 hover:text-[var(--text-primary)]"
              >
                <X className="h-5 w-5" />
              </button>
            </div>

            {formError && (
              <div className="mb-4 rounded-xl border border-[#ff446630] bg-[#ff446615] p-3 text-sm text-[#ff6680] animate-fade-in">
                {formError}
              </div>
            )}

            <div className="space-y-4">
              <div>
                <label className="mb-1.5 block text-xs font-semibold uppercase tracking-wider text-[var(--text-muted)]">
                  {t('memory.key_required')} <span className="text-[#ff4466]">*</span>
                </label>
                <input
                  type="text"
                  value={formKey}
                  onChange={(e) => setFormKey(e.target.value)}
                  placeholder="e.g. user_preferences"
                  className="input-warm w-full px-3 py-2.5 text-sm"
                  autoFocus
                />
              </div>
              <div>
                <label className="mb-1.5 block text-xs font-semibold uppercase tracking-wider text-[var(--text-muted)]">
                  {t('memory.content_required')} <span className="text-[#ff4466]">*</span>
                </label>
                <textarea
                  value={formContent}
                  onChange={(e) => setFormContent(e.target.value)}
                  placeholder="Memory content..."
                  rows={4}
                  className="input-warm w-full resize-none px-3 py-2.5 text-sm"
                />
              </div>
              <div>
                <label className="mb-1.5 block text-xs font-semibold uppercase tracking-wider text-[var(--text-muted)]">
                  {t('memory.category_optional')}
                </label>
                <input
                  type="text"
                  value={formCategory}
                  onChange={(e) => setFormCategory(e.target.value)}
                  placeholder="e.g. preferences, context, facts"
                  className="input-warm w-full px-3 py-2.5 text-sm"
                />
              </div>
            </div>

            <div className="mt-6 flex justify-end gap-3">
              <button
                onClick={() => {
                  setShowForm(false);
                  setFormError(null);
                }}
                className="rounded-xl border border-[var(--bg-secondary)] px-4 py-2 text-sm font-medium text-[var(--text-muted)] transition-all duration-300 hover:bg-[var(--glow-secondary)] hover:text-[var(--text-primary)]"
              >
                {t('memory.cancel')}
              </button>
              <button
                onClick={handleAdd}
                disabled={submitting}
                className="btn-primary px-4 py-2 text-sm font-medium"
              >
                {submitting ? t('memory.saving') : t('common.save')}
              </button>
            </div>
          </div>
        </div>,
        document.body,
      )}
    </div>
  );
}
