import { useState, useEffect, useMemo } from 'react';
import { createPortal } from 'react-dom';
import {
  Brain,
  Search,
  Plus,
  Trash2,
  X,
  Filter,
  ChevronUp,
  ChevronDown,
} from 'lucide-react';
import type { MemoryEntry } from '@/types/api';
import { getMemory, storeMemory, deleteMemory } from '@/lib/api';
import { t } from '@/lib/i18n';

function truncate(text: string, max: number): string {
  if (text.length <= max) return text;
  return text.slice(0, max) + '...';
}

function formatDate(iso: string): string {
  const d = new Date(iso);
  return d.toLocaleString();
}

type SortField = 'key' | 'category' | 'timestamp';
type SortDir = 'asc' | 'desc';

export default function Memory() {
  const [entries, setEntries] = useState<MemoryEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [search, setSearch] = useState('');
  const [categoryFilter, setCategoryFilter] = useState('');
  const [showForm, setShowForm] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [sortField, setSortField] = useState<SortField>('timestamp');
  const [sortDir, setSortDir] = useState<SortDir>('desc');

  // Form state
  const [formKey, setFormKey] = useState('');
  const [formContent, setFormContent] = useState('');
  const [formCategory, setFormCategory] = useState('');
  const [formError, setFormError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const fetchEntries = (q?: string, cat?: string) => {
    setLoading(true);
    getMemory(q || undefined, cat || undefined)
      .then(setEntries)
      .catch((err) => setError(err.message))
      .finally(() => setLoading(false));
  };

  useEffect(() => {
    fetchEntries();
  }, []);

  // Escape key closes modal
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

  // Client-side sorting
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

  if (error && entries.length === 0) {
    return (
      <div className="p-6 animate-fade-in">
        <div className="rounded-xl bg-[#ff446615] border border-[#ff446630] p-4 text-[#ff6680]">
          {t('memory.load_error')}: {error}
        </div>
      </div>
    );
  }

  return (
    <div className="p-6 space-y-6 animate-fade-in">
      <div>
        <h1 className="text-2xl font-bold text-gradient">{t('memory.title')}</h1>
        <p className="text-xs text-[var(--text-secondary)] mt-1">{t('memory.subtitle')}</p>
      </div>
      {/* Header */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          <Brain className="h-5 w-5 text-[var(--accent-primary)]" />
          <h2 className="text-sm font-semibold text-[var(--text-primary)] uppercase tracking-wider">
            {t('memory.memory_title')} ({entries.length})
          </h2>
        </div>
        <button
          onClick={() => setShowForm(true)}
          className="btn-primary flex items-center gap-2 text-sm px-4 py-2"
        >
          <Plus className="h-4 w-4" />
          {t('memory.add_memory')}
        </button>
      </div>

      {/* Search and Filter */}
      <div className="flex flex-col sm:flex-row gap-3">
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
            className="input-warm pl-10 pr-8 py-2.5 text-sm appearance-none cursor-pointer"
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

      {/* Error banner (non-fatal) */}
      {error && (
        <div className="rounded-xl bg-[#ff446615] border border-[#ff446630] p-3 text-sm text-[#ff6680] animate-fade-in">
          {error}
        </div>
      )}

      {/* Add Memory Form Modal — portaled to body to escape layout stacking context */}
      {showForm && createPortal(
        <div className="fixed inset-0 pl-60 z-[9999] flex items-center justify-center">
          <div className="absolute inset-0 bg-black/60 backdrop-blur-sm" onClick={() => { setShowForm(false); setFormError(null); }} />
          <div className="relative glass-card p-6 w-full max-w-md mx-4 animate-fade-in-scale">
            <div className="flex items-center justify-between mb-4">
              <h3 className="text-lg font-semibold text-[var(--text-primary)]">{t('memory.add_modal_title')}</h3>
              <button
                onClick={() => {
                  setShowForm(false);
                  setFormError(null);
                }}
                className="text-[var(--text-secondary)] hover:text-[var(--text-primary)] transition-colors duration-300"
              >
                <X className="h-5 w-5" />
              </button>
            </div>

            {formError && (
              <div className="mb-4 rounded-xl bg-[#ff446615] border border-[#ff446630] p-3 text-sm text-[#ff6680] animate-fade-in">
                {formError}
              </div>
            )}

            <div className="space-y-4">
              <div>
                <label className="block text-xs font-semibold text-[var(--text-muted)] mb-1.5 uppercase tracking-wider">
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
                <label className="block text-xs font-semibold text-[var(--text-muted)] mb-1.5 uppercase tracking-wider">
                  {t('memory.content_required')} <span className="text-[#ff4466]">*</span>
                </label>
                <textarea
                  value={formContent}
                  onChange={(e) => setFormContent(e.target.value)}
                  placeholder="Memory content..."
                  rows={4}
                  className="input-warm w-full px-3 py-2.5 text-sm resize-none"
                />
              </div>
              <div>
                <label className="block text-xs font-semibold text-[var(--text-muted)] mb-1.5 uppercase tracking-wider">
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

            <div className="flex justify-end gap-3 mt-6">
              <button
                onClick={() => {
                  setShowForm(false);
                  setFormError(null);
                }}
                className="px-4 py-2 text-sm font-medium text-[var(--text-muted)] hover:text-[var(--text-primary)] border border-[var(--bg-secondary)] rounded-xl hover:bg-[var(--glow-secondary)] transition-all duration-300"
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

      {/* Memory Table */}
      {loading ? (
        <div className="flex items-center justify-center h-32">
          <div className="h-8 w-8 border-2 border-[var(--glow-primary)] border-t-[var(--accent-primary)] rounded-full animate-spin" />
        </div>
      ) : entries.length === 0 ? (
        <div className="glass-card p-8 text-center">
          <Brain className="h-10 w-10 text-[var(--bg-secondary)] mx-auto mb-3" />
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
                <>
                  <tr
                    key={entry.id}
                    className="cursor-pointer hover:bg-[var(--bg-secondary)]/50 transition-colors"
                    onClick={() => setExpandedId(expandedId === entry.id ? null : entry.id)}
                  >
                    <td className="px-4 py-3 text-[var(--text-primary)] font-medium font-mono text-xs">
                      {entry.key}
                    </td>
                    <td className="px-4 py-3 text-[var(--text-muted)] max-w-[300px] text-sm">
                      <span title={entry.content}>
                        {truncate(entry.content, 80)}
                      </span>
                    </td>
                    <td className="px-4 py-3">
                      <span className="inline-flex items-center px-2.5 py-0.5 rounded-full text-[10px] font-semibold capitalize border border-[var(--bg-secondary)] text-[var(--text-muted)]" style={{ background: 'var(--glow-primary)' }}>
                        {entry.category || 'uncategorized'}
                      </span>
                    </td>
                    <td className="px-4 py-3 text-[var(--text-secondary)] text-xs whitespace-nowrap">
                      {formatDate(entry.timestamp)}
                    </td>
                    {hasScores && (
                      <td className="px-4 py-3 text-[var(--text-muted)] text-xs font-mono">
                        {entry.score != null ? `${(entry.score * 100).toFixed(0)}%` : '—'}
                      </td>
                    )}
                    <td className="px-4 py-3 text-right" onClick={(e) => e.stopPropagation()}>
                      {confirmDelete === entry.key ? (
                        <div className="flex items-center justify-end gap-2 animate-fade-in">
                          <span className="text-xs text-[#ff4466]">{t('memory.delete_confirm')}</span>
                          <button
                            onClick={() => handleDelete(entry.key)}
                            className="text-[#ff4466] hover:text-[#ff6680] text-xs font-medium"
                          >
                            {t('memory.yes')}
                          </button>
                          <button
                            onClick={() => setConfirmDelete(null)}
                            className="text-[var(--text-secondary)] hover:text-[var(--text-primary)] text-xs font-medium"
                          >
                            {t('memory.no')}
                          </button>
                        </div>
                      ) : (
                        <button
                          onClick={() => setConfirmDelete(entry.key)}
                          className="text-[var(--text-secondary)] hover:text-[#ff4466] transition-all duration-300"
                        >
                          <Trash2 className="h-4 w-4" />
                        </button>
                      )}
                    </td>
                  </tr>
                  {expandedId === entry.id && (
                    <tr key={`${entry.id}-expanded`} className="animate-fade-in">
                      <td colSpan={colCount} className="px-4 py-3 bg-[var(--bg-primary)]">
                        <pre className="whitespace-pre-wrap break-words text-sm text-[var(--text-primary)] font-mono leading-relaxed max-h-64 overflow-y-auto">
                          {entry.content}
                        </pre>
                      </td>
                    </tr>
                  )}
                </>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
