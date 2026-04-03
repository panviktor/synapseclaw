import { useState, useEffect } from 'react';
import { Lightbulb, Plus, Trash2, RotateCcw } from 'lucide-react';
import { getLearningPatterns, addLearningPattern, deleteLearningPattern, seedLearningPatterns } from '@/lib/api';
import type { SignalPattern } from '@/lib/api';

const SIGNAL_TYPES = ['correction', 'memory', 'instruction'] as const;
const MATCH_MODES = ['starts_with', 'contains'] as const;
const LANGUAGES = ['en', 'ru', 'de', 'fr', 'es', 'zh', 'ja'] as const;

const TYPE_COLORS: Record<string, string> = {
  correction: 'bg-red-100 text-red-800 dark:bg-red-900/30 dark:text-red-300',
  memory: 'bg-blue-100 text-blue-800 dark:bg-blue-900/30 dark:text-blue-300',
  instruction: 'bg-green-100 text-green-800 dark:bg-green-900/30 dark:text-green-300',
};

export default function LearningPatterns() {
  const [patterns, setPatterns] = useState<SignalPattern[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showForm, setShowForm] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const [filterType, setFilterType] = useState('');

  // Form state
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
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-bold text-gray-900 dark:text-white flex items-center gap-2">
            <Lightbulb className="w-6 h-6" />
            Learning Patterns
          </h1>
          <p className="text-sm text-gray-500 dark:text-gray-400 mt-1">
            Configure how the agent detects explicit learning signals in messages.
            Patterns are matched against the lowercased message text.
          </p>
        </div>
        <div className="flex gap-2">
          <button
            onClick={handleSeed}
            className="inline-flex items-center gap-1 px-3 py-2 text-sm bg-gray-100 hover:bg-gray-200 dark:bg-gray-700 dark:hover:bg-gray-600 rounded-md"
            title="Restore default patterns (only if table is empty)"
          >
            <RotateCcw className="w-4 h-4" />
            Seed defaults
          </button>
          <button
            onClick={() => setShowForm(!showForm)}
            className="inline-flex items-center gap-1 px-3 py-2 text-sm bg-blue-600 hover:bg-blue-700 text-white rounded-md"
          >
            <Plus className="w-4 h-4" />
            Add pattern
          </button>
        </div>
      </div>

      {error && (
        <div className="bg-red-50 dark:bg-red-900/20 text-red-700 dark:text-red-300 p-3 rounded-md text-sm">
          {error}
          <button onClick={() => setError(null)} className="ml-2 underline">dismiss</button>
        </div>
      )}

      {/* Add form */}
      {showForm && (
        <div className="bg-white dark:bg-gray-800 border border-gray-200 dark:border-gray-700 rounded-lg p-4 space-y-3">
          <div className="grid grid-cols-2 md:grid-cols-4 gap-3">
            <div>
              <label className="block text-xs font-medium text-gray-500 mb-1">Signal type</label>
              <select
                value={formType}
                onChange={(e) => setFormType(e.target.value)}
                className="w-full px-2 py-1.5 text-sm border rounded-md dark:bg-gray-700 dark:border-gray-600"
              >
                {SIGNAL_TYPES.map((t) => (
                  <option key={t} value={t}>{t}</option>
                ))}
              </select>
            </div>
            <div>
              <label className="block text-xs font-medium text-gray-500 mb-1">Match mode</label>
              <select
                value={formMode}
                onChange={(e) => setFormMode(e.target.value)}
                className="w-full px-2 py-1.5 text-sm border rounded-md dark:bg-gray-700 dark:border-gray-600"
              >
                {MATCH_MODES.map((m) => (
                  <option key={m} value={m}>{m.replace('_', ' ')}</option>
                ))}
              </select>
            </div>
            <div>
              <label className="block text-xs font-medium text-gray-500 mb-1">Language</label>
              <select
                value={formLang}
                onChange={(e) => setFormLang(e.target.value)}
                className="w-full px-2 py-1.5 text-sm border rounded-md dark:bg-gray-700 dark:border-gray-600"
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
                className="w-full px-3 py-1.5 text-sm bg-blue-600 hover:bg-blue-700 disabled:opacity-50 text-white rounded-md"
              >
                {submitting ? 'Adding...' : 'Add'}
              </button>
            </div>
          </div>
          <div>
            <label className="block text-xs font-medium text-gray-500 mb-1">
              Pattern text (lowercase, e.g. "remember ", "actually,")
            </label>
            <input
              type="text"
              value={formPattern}
              onChange={(e) => setFormPattern(e.target.value)}
              placeholder="remember "
              className="w-full px-3 py-2 text-sm border rounded-md dark:bg-gray-700 dark:border-gray-600"
              onKeyDown={(e) => e.key === 'Enter' && handleAdd()}
            />
          </div>
        </div>
      )}

      {/* Filter */}
      <div className="flex gap-2">
        <button
          onClick={() => setFilterType('')}
          className={`px-3 py-1 text-xs rounded-full ${!filterType ? 'bg-gray-900 text-white dark:bg-white dark:text-gray-900' : 'bg-gray-100 dark:bg-gray-700'}`}
        >
          All ({patterns.length})
        </button>
        {SIGNAL_TYPES.map((t) => (
          <button
            key={t}
            onClick={() => setFilterType(t)}
            className={`px-3 py-1 text-xs rounded-full ${filterType === t ? 'bg-gray-900 text-white dark:bg-white dark:text-gray-900' : TYPE_COLORS[t]}`}
          >
            {t} ({patterns.filter((p) => p.signal_type === t).length})
          </button>
        ))}
      </div>

      {/* Pattern list */}
      {loading ? (
        <p className="text-gray-500 text-sm">Loading...</p>
      ) : filtered.length === 0 ? (
        <div className="text-center py-12 text-gray-400">
          <Lightbulb className="w-12 h-12 mx-auto mb-3 opacity-30" />
          <p>No patterns configured. Click "Seed defaults" to get started.</p>
        </div>
      ) : (
        <div className="bg-white dark:bg-gray-800 border border-gray-200 dark:border-gray-700 rounded-lg overflow-hidden">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-gray-200 dark:border-gray-700 bg-gray-50 dark:bg-gray-750">
                <th className="px-4 py-2 text-left text-xs font-medium text-gray-500">Type</th>
                <th className="px-4 py-2 text-left text-xs font-medium text-gray-500">Pattern</th>
                <th className="px-4 py-2 text-left text-xs font-medium text-gray-500">Match</th>
                <th className="px-4 py-2 text-left text-xs font-medium text-gray-500">Lang</th>
                <th className="px-4 py-2 text-right text-xs font-medium text-gray-500">Actions</th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((p) => (
                <tr key={p.id} className="border-b border-gray-100 dark:border-gray-700/50 hover:bg-gray-50 dark:hover:bg-gray-750">
                  <td className="px-4 py-2">
                    <span className={`px-2 py-0.5 text-xs rounded-full ${TYPE_COLORS[p.signal_type] || 'bg-gray-100'}`}>
                      {p.signal_type}
                    </span>
                  </td>
                  <td className="px-4 py-2 font-mono text-xs">
                    {p.pattern}
                  </td>
                  <td className="px-4 py-2 text-xs text-gray-500">
                    {p.match_mode.replace('_', ' ')}
                  </td>
                  <td className="px-4 py-2 text-xs text-gray-500">{p.language}</td>
                  <td className="px-4 py-2 text-right">
                    {confirmDelete === p.id ? (
                      <span className="text-xs">
                        <button onClick={() => handleDelete(p.id)} className="text-red-600 hover:underline mr-2">confirm</button>
                        <button onClick={() => setConfirmDelete(null)} className="text-gray-400 hover:underline">cancel</button>
                      </span>
                    ) : (
                      <button
                        onClick={() => setConfirmDelete(p.id)}
                        className="text-gray-400 hover:text-red-500"
                      >
                        <Trash2 className="w-4 h-4" />
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
