import { useState, useCallback } from 'react';
import { apiFetch } from '@/lib/api';

interface DeadLetter {
  id: string;
  pipeline_run_id: string;
  step_id: string;
  agent_id: string;
  input: unknown;
  error: string;
  attempt: number;
  max_retries: number;
  created_at: number;
  status: string;
  retried_at?: number;
  dismissed_by?: string;
}

export default function DeadLetters() {
  const [letters, setLetters] = useState<DeadLetter[]>([]);
  const [loading, setLoading] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [showAll, setShowAll] = useState(false);
  const [expandedId, setExpandedId] = useState<string | null>(null);

  const doSearch = useCallback(async () => {
    setLoading(true);
    try {
      const params = new URLSearchParams({ limit: '100' });
      if (showAll) params.set('all', 'true');
      const data = await apiFetch<{ dead_letters: DeadLetter[] }>(`/api/pipelines/dead-letters?${params}`);
      setLetters(data.dead_letters || []);
      setLoaded(true);
    } catch {
      setLetters([]);
      setLoaded(true);
    } finally {
      setLoading(false);
    }
  }, [showAll]);

  const handleRetry = async (id: string) => {
    try {
      await apiFetch(`/api/pipelines/dead-letters/${id}/retry`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' });
      doSearch();
    } catch (e) {
      console.error('Retry failed:', e);
    }
  };

  const handleDismiss = async (id: string) => {
    try {
      await apiFetch(`/api/pipelines/dead-letters/${id}/dismiss`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ dismissed_by: 'dashboard' }),
      });
      doSearch();
    } catch (e) {
      console.error('Dismiss failed:', e);
    }
  };

  const formatTime = (ts: number) => {
    if (!ts) return '-';
    return new Date(ts * 1000).toLocaleString();
  };

  const statusColor = (status: string) => {
    switch (status) {
      case 'pending': return 'text-yellow-400';
      case 'retried': return 'text-blue-400';
      case 'dismissed': return 'text-theme-muted';
      default: return 'text-theme-default';
    }
  };

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <h1 className="text-xl font-semibold text-theme-default">Dead Letters</h1>
        <div className="flex items-center gap-3">
          <label className="flex items-center gap-2 text-sm text-theme-muted">
            <input
              type="checkbox"
              checked={showAll}
              onChange={(e) => setShowAll(e.target.checked)}
              className="rounded"
            />
            Show all
          </label>
          <button
            onClick={() => doSearch()}
            disabled={loading}
            className="btn-primary px-4 py-2 text-sm"
          >
            {loading ? 'Loading...' : 'Load'}
          </button>
        </div>
      </div>

      {loaded && letters.length === 0 && (
        <div className="text-center py-12 text-theme-muted">
          No dead letters found.
        </div>
      )}

      {letters.length > 0 && (
        <div className="overflow-x-auto">
          <table className="w-full text-sm">
            <thead>
              <tr className="text-theme-muted text-left border-b border-theme-default">
                <th className="pb-2 pr-4">Step</th>
                <th className="pb-2 pr-4">Agent</th>
                <th className="pb-2 pr-4">Error</th>
                <th className="pb-2 pr-4">Attempts</th>
                <th className="pb-2 pr-4">Created</th>
                <th className="pb-2 pr-4">Status</th>
                <th className="pb-2">Actions</th>
              </tr>
            </thead>
            <tbody>
              {letters.map((dl) => (
                <tr
                  key={dl.id}
                  className="border-b border-theme-default/50 hover:bg-theme-secondary/30 cursor-pointer"
                  onClick={() => setExpandedId(expandedId === dl.id ? null : dl.id)}
                >
                  <td className="py-2 pr-4 font-mono text-xs">{dl.step_id}</td>
                  <td className="py-2 pr-4">{dl.agent_id}</td>
                  <td className="py-2 pr-4 max-w-xs truncate" title={dl.error}>
                    {dl.error}
                  </td>
                  <td className="py-2 pr-4">{dl.attempt}/{dl.max_retries + 1}</td>
                  <td className="py-2 pr-4 text-xs text-theme-muted">
                    {formatTime(dl.created_at)}
                  </td>
                  <td className={`py-2 pr-4 font-medium ${statusColor(dl.status)}`}>
                    {dl.status}
                  </td>
                  <td className="py-2">
                    {dl.status === 'pending' && (
                      <div className="flex gap-2" onClick={(e) => e.stopPropagation()}>
                        <button
                          onClick={() => handleRetry(dl.id)}
                          className="px-2 py-1 text-xs bg-blue-600 hover:bg-blue-700 text-white rounded"
                        >
                          Retry
                        </button>
                        <button
                          onClick={() => handleDismiss(dl.id)}
                          className="px-2 py-1 text-xs bg-theme-secondary hover:bg-theme-secondary/80 text-theme-muted rounded"
                        >
                          Dismiss
                        </button>
                      </div>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {expandedId && (() => {
        const dl = letters.find(l => l.id === expandedId);
        if (!dl) return null;
        return (
          <div className="bg-theme-card border border-theme-default rounded-xl p-4 mt-4">
            <h3 className="text-sm font-semibold mb-2 text-theme-default">Dead Letter Details</h3>
            <div className="grid grid-cols-2 gap-2 text-xs mb-3">
              <div><span className="text-theme-muted">ID:</span> {dl.id}</div>
              <div><span className="text-theme-muted">Pipeline Run:</span> {dl.pipeline_run_id}</div>
              <div><span className="text-theme-muted">Retried At:</span> {dl.retried_at ? formatTime(dl.retried_at) : '-'}</div>
              <div><span className="text-theme-muted">Dismissed By:</span> {dl.dismissed_by || '-'}</div>
            </div>
            <div className="text-xs">
              <span className="text-theme-muted">Input:</span>
              <pre className="mt-1 bg-theme-secondary rounded p-2 overflow-x-auto max-h-40 text-theme-default">
                {JSON.stringify(dl.input, null, 2)}
              </pre>
            </div>
            <div className="text-xs mt-2">
              <span className="text-theme-muted">Error:</span>
              <pre className="mt-1 bg-theme-secondary rounded p-2 overflow-x-auto max-h-40 text-status-error">
                {dl.error}
              </pre>
            </div>
          </div>
        );
      })()}
    </div>
  );
}
