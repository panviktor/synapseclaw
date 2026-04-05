import { Fragment, useState, useEffect, useMemo, useCallback } from 'react';
import { createPortal } from 'react-dom';
import { useNavigate, useSearchParams } from 'react-router-dom';
import {
  Brain,
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
} from 'lucide-react';
import type { ContextBudgetResponse, MemoryEntry, MemoryStatsResponse } from '@/types/api';
import { getMemory, storeMemory, deleteMemory, getMemoryStats, getContextBudget } from '@/lib/api';
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
type StudioTab = 'overview' | 'blocks' | 'budget' | 'entries';

function StudioTabButton({
  active,
  label,
  onClick,
}: {
  active: boolean;
  label: string;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      className={`rounded-full px-3 py-1.5 text-xs font-semibold uppercase tracking-wide transition-all ${
        active
          ? 'bg-[var(--accent-primary)] text-white shadow-[0_8px_24px_var(--glow-primary)]'
          : 'border border-[var(--border-default)] bg-[var(--bg-card)] text-[var(--text-muted)] hover:text-[var(--text-primary)]'
      }`}
    >
      {label}
    </button>
  );
}

function StatCard({
  label,
  value,
  caption,
}: {
  label: string;
  value: string;
  caption: string;
}) {
  return (
    <div className="rounded-3xl border border-[var(--border-default)] bg-[var(--bg-card)] px-4 py-4">
      <p className="text-[10px] uppercase tracking-[0.24em] text-[var(--text-placeholder)]">{label}</p>
      <p className="mt-2 text-2xl font-semibold tracking-tight text-[var(--text-primary)]">{value}</p>
      <p className="mt-1 text-xs text-[var(--text-muted)]">{caption}</p>
    </div>
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
  const [search, setSearch] = useState('');
  const [categoryFilter, setCategoryFilter] = useState('');
  const [showForm, setShowForm] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [sortField, setSortField] = useState<SortField>('timestamp');
  const [sortDir, setSortDir] = useState<SortDir>('desc');
  const [activeTab, setActiveTab] = useState<StudioTab>('overview');

  const [formKey, setFormKey] = useState('');
  const [formContent, setFormContent] = useState('');
  const [formCategory, setFormCategory] = useState('');
  const [formError, setFormError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const agentLabel = stats?.agent_id ?? selectedAgent ?? 'Local Runtime';
  const topCategories = stats?.by_category.slice(0, 5) ?? [];

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
    ])
      .then(([memoryStats, contextBudget]) => {
        setStats(memoryStats);
        setBudget(contextBudget);
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
      <div className="relative overflow-hidden rounded-[28px] border border-[var(--border-default)] bg-[linear-gradient(135deg,var(--glow-primary),transparent_35%),var(--bg-card)] px-6 py-6">
        <div className="absolute inset-x-10 top-0 h-px bg-gradient-to-r from-transparent via-[var(--accent-primary)]/70 to-transparent" />
        <div className="flex flex-col gap-5 xl:flex-row xl:items-end xl:justify-between">
          <div>
            <p className="text-[11px] font-semibold uppercase tracking-[0.28em] text-[var(--text-placeholder)]">
              Memory Studio
            </p>
            <div className="mt-2 flex flex-wrap items-center gap-2">
              <h1 className="text-3xl font-semibold tracking-tight text-[var(--text-primary)]">
                {agentLabel}
              </h1>
              <span className={`rounded-full px-2.5 py-1 text-[10px] font-semibold uppercase tracking-wide ${
                remoteScope
                  ? 'bg-[var(--status-info)]/10 text-[var(--status-info)]'
                  : 'bg-[var(--status-success)]/12 text-[var(--status-success)]'
              }`}>
                {remoteScope ? 'remote scope' : 'local scope'}
              </span>
            </div>
            <p className="mt-2 max-w-2xl text-sm text-[var(--text-muted)]">
              Inspect long-term memory shape, core blocks, prompt budget, and learning surface without dropping back to raw tables.
            </p>
          </div>

          <div className="flex flex-wrap gap-2">
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

      {error && (
        <div className="rounded-xl border border-[#ff446630] bg-[#ff446615] p-3 text-sm text-[#ff6680] animate-fade-in">
          {error}
        </div>
      )}

      <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-4">
        <StatCard
          label="Entries"
          value={surfaceLoading ? '…' : formatNumber(stats?.total_entries ?? 0)}
          caption="Total surfaced memories"
        />
        <StatCard
          label="Skills"
          value={surfaceLoading ? '…' : formatNumber(stats?.skills ?? 0)}
          caption="Procedural patterns available"
        />
        <StatCard
          label="Entities"
          value={surfaceLoading ? '…' : formatNumber(stats?.entities ?? 0)}
          caption="Knowledge graph nodes"
        />
        <StatCard
          label="Mode"
          value={surfaceLoading ? '…' : (budget?.continuation_policy ?? 'n/a')}
          caption="Continuation and recall policy"
        />
      </div>

      <div className="flex flex-wrap gap-2">
        <StudioTabButton active={activeTab === 'overview'} label="Overview" onClick={() => setActiveTab('overview')} />
        <StudioTabButton active={activeTab === 'blocks'} label="Core Blocks" onClick={() => setActiveTab('blocks')} />
        <StudioTabButton active={activeTab === 'budget'} label="Budget" onClick={() => setActiveTab('budget')} />
        <StudioTabButton active={activeTab === 'entries'} label="Entries" onClick={() => setActiveTab('entries')} />
      </div>

      {activeTab === 'overview' && (
        <div className="grid gap-6 xl:grid-cols-[1.3fr_0.7fr]">
          <section className="glass-card overflow-hidden">
            <div className="border-b border-[var(--border-default)] px-5 py-4">
              <div className="flex items-center gap-2">
                <Layers3 className="h-4 w-4 text-[var(--accent-primary)]" />
                <h2 className="text-sm font-semibold text-[var(--text-primary)]">Memory Surface</h2>
              </div>
            </div>
            <div className="space-y-4 px-5 py-5">
              {topCategories.length === 0 ? (
                <p className="text-sm text-[var(--text-muted)]">
                  No category distribution has been surfaced yet.
                </p>
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
          </section>

          <section className="space-y-4">
            <div className="glass-card overflow-hidden">
              <div className="border-b border-[var(--border-default)] px-5 py-4">
                <div className="flex items-center gap-2">
                  <Sparkles className="h-4 w-4 text-[var(--accent-primary)]" />
                  <h2 className="text-sm font-semibold text-[var(--text-primary)]">Quick Paths</h2>
                </div>
              </div>
              <div className="space-y-3 px-5 py-5">
                <button
                  onClick={() => navigate(remoteScope ? `/agents?agent=${encodeURIComponent(selectedAgent ?? '')}` : '/agents')}
                  className="flex w-full items-center justify-between rounded-2xl border border-[var(--border-default)] bg-[var(--bg-card)] px-4 py-3 text-left transition-colors hover:border-[var(--accent-primary)]/30"
                >
                  <div>
                    <p className="text-sm font-medium text-[var(--text-primary)]">Agent Workbench</p>
                    <p className="mt-1 text-xs text-[var(--text-muted)]">Jump back into the live transcript and memory pulse.</p>
                  </div>
                  <ArrowRight className="h-4 w-4 text-[var(--text-muted)]" />
                </button>
              </div>
            </div>

            <div className="glass-card overflow-hidden">
              <div className="border-b border-[var(--border-default)] px-5 py-4">
                <div className="flex items-center gap-2">
                  <Gauge className="h-4 w-4 text-[var(--accent-primary)]" />
                  <h2 className="text-sm font-semibold text-[var(--text-primary)]">Budget Snapshot</h2>
                </div>
              </div>
              <div className="space-y-3 px-5 py-5 text-sm">
                <div className="rounded-2xl border border-[var(--border-default)] bg-[var(--bg-card)] px-4 py-3">
                  <p className="text-[10px] uppercase tracking-[0.22em] text-[var(--text-placeholder)]">Continuation</p>
                  <p className="mt-1 font-medium text-[var(--text-primary)]">{budget?.continuation_policy ?? 'n/a'}</p>
                </div>
                <div className="rounded-2xl border border-[var(--border-default)] bg-[var(--bg-card)] px-4 py-3">
                  <p className="text-[10px] uppercase tracking-[0.22em] text-[var(--text-placeholder)]">Envelope</p>
                  <p className="mt-1 font-medium text-[var(--text-primary)]">
                    {budget ? formatChars(budget.enrichment_total_max_chars) : 'n/a'}
                  </p>
                </div>
              </div>
            </div>
          </section>
        </div>
      )}

      {activeTab === 'blocks' && (
        <section className="glass-card overflow-hidden">
          <div className="border-b border-[var(--border-default)] px-5 py-4">
            <div className="flex items-center gap-2">
              <BookMarked className="h-4 w-4 text-[var(--accent-primary)]" />
              <h2 className="text-sm font-semibold text-[var(--text-primary)]">Core Blocks</h2>
            </div>
          </div>
          <div className="grid gap-4 px-5 py-5 md:grid-cols-2">
            {stats?.core_blocks.length ? (
              stats.core_blocks.map((block) => (
                <div key={block.label} className="rounded-3xl border border-[var(--border-default)] bg-[var(--bg-card)] px-4 py-4">
                  <p className="text-[10px] uppercase tracking-[0.22em] text-[var(--text-placeholder)]">
                    {block.label.replace(/_/g, ' ')}
                  </p>
                  <p className="mt-2 text-lg font-semibold text-[var(--text-primary)]">{formatChars(block.chars)}</p>
                  <p className="mt-1 text-xs text-[var(--text-muted)]">Updated {formatDate(block.updated_at)}</p>
                </div>
              ))
            ) : (
              <p className="text-sm text-[var(--text-muted)]">No core blocks have been surfaced for this agent yet.</p>
            )}
          </div>
        </section>
      )}

      {activeTab === 'budget' && (
        <section className="glass-card overflow-hidden">
          <div className="border-b border-[var(--border-default)] px-5 py-4">
            <div className="flex items-center gap-2">
              <Gauge className="h-4 w-4 text-[var(--accent-primary)]" />
              <h2 className="text-sm font-semibold text-[var(--text-primary)]">Context Budget</h2>
            </div>
          </div>
          <div className="space-y-5 px-5 py-5">
            {budget ? (
              <>
                {[
                  {
                    label: 'Recall',
                    share: budgetShare(budget.recall_total_max_chars, budget.enrichment_total_max_chars),
                    meta: `${budget.recall_max_entries} entries · ${formatChars(budget.recall_total_max_chars)}`,
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

                <div className="grid gap-4 md:grid-cols-2">
                  <div className="rounded-3xl border border-[var(--border-default)] bg-[var(--bg-card)] px-4 py-4">
                    <p className="text-[10px] uppercase tracking-[0.22em] text-[var(--text-placeholder)]">Continuation policy</p>
                    <p className="mt-2 text-lg font-semibold text-[var(--text-primary)]">{budget.continuation_policy}</p>
                  </div>
                  <div className="rounded-3xl border border-[var(--border-default)] bg-[var(--bg-card)] px-4 py-4">
                    <p className="text-[10px] uppercase tracking-[0.22em] text-[var(--text-placeholder)]">Min relevance</p>
                    <p className="mt-2 text-lg font-semibold text-[var(--text-primary)]">{budget.min_relevance_score.toFixed(2)}</p>
                  </div>
                </div>
              </>
            ) : (
              <p className="text-sm text-[var(--text-muted)]">Context budget is not available for this scope yet.</p>
            )}
          </div>
        </section>
      )}

      {activeTab === 'entries' && (
        <div className="space-y-6">
          {remoteScope ? (
            <div className="glass-card px-6 py-8 text-center">
              <Brain className="mx-auto mb-4 h-10 w-10 text-[var(--accent-primary)]" />
              <p className="text-lg font-semibold text-[var(--text-primary)]">Remote entry inspector is not proxied yet</p>
              <p className="mx-auto mt-2 max-w-2xl text-sm text-[var(--text-muted)]">
                This view already uses remote `stats` and `context budget`, but raw entry list, add, and delete are still local-only surfaces.
              </p>
            </div>
          ) : (
            <>
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
                <div className="glass-card p-8 text-center">
                  <Brain className="mx-auto mb-3 h-10 w-10 text-[var(--bg-secondary)]" />
                  <p className="text-[var(--text-secondary)]">{t('memory.empty')}</p>
                </div>
              ) : (
                <div className="glass-card overflow-x-auto">
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
          <div className="relative glass-card mx-4 w-full max-w-md p-6 animate-fade-in-scale">
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
