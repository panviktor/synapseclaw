import { useState, useEffect } from 'react';
import { Lightbulb, Plus, Trash2, RotateCcw, Sparkles, Wand2 } from 'lucide-react';
import { getLearningPatterns, addLearningPattern, deleteLearningPattern, seedLearningPatterns } from '@/lib/api';
import type { SignalPattern } from '@/lib/api';

const SIGNAL_TYPES = ['correction', 'memory', 'instruction'] as const;
const MATCH_MODES = ['starts_with', 'contains'] as const;
const LANGUAGES = ['en', 'ru', 'de', 'fr', 'es', 'zh', 'ja'] as const;

const TYPE_CLASSES: Record<string, string> = {
  correction: 'bg-[var(--status-error)]/10 text-[var(--status-error)]',
  memory: 'bg-[var(--status-info)]/10 text-[var(--status-info)]',
  instruction: 'bg-[var(--status-success)]/10 text-[var(--status-success)]',
};

function StatPill({ label, value }: { label: string; value: number }) {
  return (
    <div className="rounded-2xl border border-[var(--border-default)] bg-[var(--bg-card)] px-4 py-3">
      <p className="text-[10px] uppercase tracking-[0.22em] text-[var(--text-placeholder)]">{label}</p>
      <p className="mt-1 text-xl font-semibold text-[var(--text-primary)]">{value}</p>
    </div>
  );
}

export default function LearningPatterns() {
  const [patterns, setPatterns] = useState<SignalPattern[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showForm, setShowForm] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const [filterType, setFilterType] = useState('');

  const [formType, setFormType] = useState<string>('memory');
  const [formPattern, setFormPattern] = useState('');
  const [formMode, setFormMode] = useState<string>('starts_with');
  const [formLang, setFormLang] = useState('en');
  const [submitting, setSubmitting] = useState(false);

  const fetchPatterns = () => {
    setLoading(true);
    getLearningPatterns()
      .then(setPatterns)
      .catch((e) => setError(e.message))
      .finally(() => setLoading(false));
  };

  useEffect(() => {
    fetchPatterns();
  }, []);

  const handleAdd = async () => {
    if (!formPattern.trim()) return;
    setSubmitting(true);
    try {
      await addLearningPattern({
        signal_type: formType,
        pattern: formPattern.toLowerCase().trim(),
        match_mode: formMode,
        language: formLang,
        enabled: true,
      });
      setFormPattern('');
      setShowForm(false);
      fetchPatterns();
    } catch (e: any) {
      setError(e.message);
    } finally {
      setSubmitting(false);
    }
  };

  const handleDelete = async (id: string) => {
    try {
      await deleteLearningPattern(id);
      setConfirmDelete(null);
      fetchPatterns();
    } catch (e: any) {
      setError(e.message);
    }
  };

  const handleSeed = async () => {
    try {
      const result = await seedLearningPatterns();
      if (result.seeded > 0) {
        fetchPatterns();
      }
    } catch (e: any) {
      setError(e.message);
    }
  };

  const filtered = filterType
    ? patterns.filter((p) => p.signal_type === filterType)
    : patterns;

  return (
    <div className="space-y-6 p-6 animate-fade-in">
      <div className="relative overflow-hidden rounded-[28px] border border-[var(--border-default)] bg-[linear-gradient(135deg,var(--glow-primary),transparent_35%),var(--bg-card)] px-6 py-6">
        <div className="absolute inset-x-10 top-0 h-px bg-gradient-to-r from-transparent via-[var(--accent-primary)]/70 to-transparent" />
        <div className="flex flex-col gap-5 xl:flex-row xl:items-end xl:justify-between">
          <div>
            <p className="text-[11px] font-semibold uppercase tracking-[0.28em] text-[var(--text-placeholder)]">
              Learning Signals
            </p>
            <div className="mt-2 flex items-center gap-2">
              <Lightbulb className="h-6 w-6 text-[var(--accent-primary)]" />
              <h1 className="text-3xl font-semibold tracking-tight text-[var(--text-primary)]">
                Pattern Registry
              </h1>
            </div>
            <p className="mt-2 max-w-2xl text-sm text-[var(--text-muted)]">
              Control how explicit learning cues are detected before memory mutation, consolidation, and reflection paths kick in.
            </p>
          </div>

          <div className="flex flex-wrap gap-2">
            <button
              onClick={handleSeed}
              className="btn-secondary inline-flex items-center gap-2 px-4 py-2 text-sm"
              title="Restore default patterns if the table is empty"
            >
              <RotateCcw className="h-4 w-4" />
              Seed Defaults
            </button>
            <button
              onClick={() => setShowForm((open) => !open)}
              className="btn-primary inline-flex items-center gap-2 px-4 py-2 text-sm"
            >
              <Plus className="h-4 w-4" />
              Add Pattern
            </button>
          </div>
        </div>
      </div>

      {error && (
        <div className="rounded-xl border border-[#ff446630] bg-[#ff446615] p-3 text-sm text-[#ff6680]">
          {error}
          <button onClick={() => setError(null)} className="ml-2 underline">dismiss</button>
        </div>
      )}

      <div className="grid gap-3 md:grid-cols-3">
        <StatPill label="Total" value={patterns.length} />
        <StatPill label="Corrections" value={patterns.filter((p) => p.signal_type === 'correction').length} />
        <StatPill label="Memory / Instruction" value={patterns.filter((p) => p.signal_type !== 'correction').length} />
      </div>

      {showForm && (
        <div className="glass-card overflow-hidden">
          <div className="border-b border-[var(--border-default)] px-5 py-4">
            <div className="flex items-center gap-2">
              <Wand2 className="h-4 w-4 text-[var(--accent-primary)]" />
              <h2 className="text-sm font-semibold text-[var(--text-primary)]">New Learning Pattern</h2>
            </div>
          </div>
          <div className="space-y-4 px-5 py-5">
            <div className="grid gap-4 md:grid-cols-4">
              <div>
                <label className="mb-1.5 block text-xs font-semibold uppercase tracking-[0.22em] text-[var(--text-placeholder)]">Signal type</label>
                <select
                  value={formType}
                  onChange={(e) => setFormType(e.target.value)}
                  className="input-warm w-full px-3 py-2 text-sm"
                >
                  {SIGNAL_TYPES.map((t) => (
                    <option key={t} value={t}>{t}</option>
                  ))}
                </select>
              </div>
              <div>
                <label className="mb-1.5 block text-xs font-semibold uppercase tracking-[0.22em] text-[var(--text-placeholder)]">Match mode</label>
                <select
                  value={formMode}
                  onChange={(e) => setFormMode(e.target.value)}
                  className="input-warm w-full px-3 py-2 text-sm"
                >
                  {MATCH_MODES.map((m) => (
                    <option key={m} value={m}>{m.replace('_', ' ')}</option>
                  ))}
                </select>
              </div>
              <div>
                <label className="mb-1.5 block text-xs font-semibold uppercase tracking-[0.22em] text-[var(--text-placeholder)]">Language</label>
                <select
                  value={formLang}
                  onChange={(e) => setFormLang(e.target.value)}
                  className="input-warm w-full px-3 py-2 text-sm"
                >
                  {LANGUAGES.map((l) => (
                    <option key={l} value={l}>{l}</option>
                  ))}
                </select>
              </div>
              <div className="flex items-end">
                <button
                  onClick={handleAdd}
                  disabled={submitting || !formPattern.trim()}
                  className="btn-primary w-full px-3 py-2 text-sm disabled:opacity-50"
                >
                  {submitting ? 'Adding...' : 'Add'}
                </button>
              </div>
            </div>
            <div>
              <label className="mb-1.5 block text-xs font-semibold uppercase tracking-[0.22em] text-[var(--text-placeholder)]">
                Pattern text
              </label>
              <input
                type="text"
                value={formPattern}
                onChange={(e) => setFormPattern(e.target.value)}
                placeholder='e.g. "remember ", "actually,"'
                className="input-warm w-full px-3 py-2.5 text-sm"
                onKeyDown={(e) => e.key === 'Enter' && handleAdd()}
              />
            </div>
          </div>
        </div>
      )}

      <div className="flex flex-wrap gap-2">
        <button
          onClick={() => setFilterType('')}
          className={`rounded-full px-3 py-1.5 text-xs font-semibold uppercase tracking-wide ${
            !filterType
              ? 'bg-[var(--accent-primary)] text-white'
              : 'border border-[var(--border-default)] bg-[var(--bg-card)] text-[var(--text-muted)]'
          }`}
        >
          All ({patterns.length})
        </button>
        {SIGNAL_TYPES.map((t) => (
          <button
            key={t}
            onClick={() => setFilterType(t)}
            className={`rounded-full px-3 py-1.5 text-xs font-semibold uppercase tracking-wide ${
              filterType === t
                ? 'bg-[var(--accent-primary)] text-white'
                : TYPE_CLASSES[t]
            }`}
          >
            {t} ({patterns.filter((p) => p.signal_type === t).length})
          </button>
        ))}
      </div>

      {loading ? (
        <div className="flex h-32 items-center justify-center">
          <div className="h-8 w-8 rounded-full border-2 border-[var(--accent-primary)]/20 border-t-[var(--accent-primary)] animate-spin" />
        </div>
      ) : filtered.length === 0 ? (
        <div className="glass-card px-6 py-12 text-center">
          <Sparkles className="mx-auto mb-3 h-12 w-12 text-[var(--accent-primary)]/40" />
          <p className="text-lg font-semibold text-[var(--text-primary)]">No patterns configured</p>
          <p className="mt-2 text-sm text-[var(--text-muted)]">Seed the defaults or add your own language-specific learning cues.</p>
        </div>
      ) : (
        <div className="glass-card overflow-hidden">
          <table className="table-warm">
            <thead>
              <tr>
                <th className="text-left">Type</th>
                <th className="text-left">Pattern</th>
                <th className="text-left">Match</th>
                <th className="text-left">Lang</th>
                <th className="text-right">Actions</th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((p) => (
                <tr key={p.id} className="hover:bg-[var(--bg-secondary)]/40 transition-colors">
                  <td className="px-4 py-3">
                    <span className={`rounded-full px-2.5 py-1 text-[10px] font-semibold uppercase tracking-wide ${TYPE_CLASSES[p.signal_type] || 'bg-[var(--bg-hover)] text-[var(--text-muted)]'}`}>
                      {p.signal_type}
                    </span>
                  </td>
                  <td className="px-4 py-3">
                    <code className="rounded-lg bg-[var(--bg-primary)] px-2 py-1 text-xs text-[var(--text-primary)]">
                      {p.pattern}
                    </code>
                  </td>
                  <td className="px-4 py-3 text-sm text-[var(--text-muted)]">
                    {p.match_mode.replace('_', ' ')}
                  </td>
                  <td className="px-4 py-3 text-sm text-[var(--text-muted)]">{p.language}</td>
                  <td className="px-4 py-3 text-right">
                    {confirmDelete === p.id ? (
                      <span className="text-xs">
                        <button onClick={() => handleDelete(p.id)} className="mr-2 text-[var(--status-error)] hover:underline">confirm</button>
                        <button onClick={() => setConfirmDelete(null)} className="text-[var(--text-muted)] hover:underline">cancel</button>
                      </span>
                    ) : (
                      <button
                        onClick={() => setConfirmDelete(p.id)}
                        className="text-[var(--text-muted)] transition-colors hover:text-[var(--status-error)]"
                      >
                        <Trash2 className="h-4 w-4" />
                      </button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
