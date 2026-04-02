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

  const statusBadge = (status: string) => {
    switch (status) {
      case 'pending': return 'bg-[#ffaa2215] text-[#ffaa44] border-[#ffaa2230]';
      case 'retried': return 'bg-[#4488ff15] text-[#4488ff] border-[#4488ff30]';
      case 'dismissed': return 'bg-[var(--glow-secondary)] text-[var(--text-muted)] border-[var(--bg-secondary)]';
      default: return 'bg-[var(--glow-secondary)] text-[var(--text-secondary)] border-[var(--bg-secondary)]';
    }
  };

  return (
    <div className="p-6 space-y-6 animate-fade-in">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-xl font-bold text-gradient">Dead Letters</h1>
          <p className="text-xs text-[var(--text-secondary)] mt-1">Failed pipeline steps awaiting retry or dismissal</p>
        </div>
        <div className="flex items-center gap-2">
          <label className="flex items-center gap-2 text-sm text-[var(--text-muted)] cursor-pointer">
            <input
              type="checkbox"
              checked={showAll}
              onChange={(e) => setShowAll(e.target.checked)}
              className="rounded border-[var(--bg-secondary)] bg-[var(--bg-card)]"
            />
            Show all
          </label>
          <button
            onClick={() => doSearch()}
            disabled={loading}
            className="px-3 py-1.5 text-sm bg-[var(--glow-primary)] hover:bg-[var(--glow-primary)] text-[var(--accent-primary)] rounded-lg transition-colors"
          >
            {loading ? 'Loading...' : 'Refresh'}
          </button>
        </div>
      </div>

      {loaded && letters.length === 0 && (
        <div className="glass-card p-8 text-center">
          <p className="text-[var(--text-secondary)]">No dead letters found.</p>
        </div>
      )}

      {letters.length > 0 && (
        <div className="glass-card overflow-x-auto">
          <table className="table-warm">
            <thead>
              <tr>
                <th className="text-left">Step</th>
                <th className="text-left">Agent</th>
                <th className="text-left">Error</th>
                <th className="text-left">Attempts</th>
                <th className="text-left">Created</th>
                <th className="text-left">Status</th>
                <th className="text-right">Actions</th>
              </tr>
            </thead>
            <tbody>
              {letters.map((dl) => (
                <tr
                  key={dl.id}
                  className="cursor-pointer"
                  onClick={() => setExpandedId(expandedId === dl.id ? null : dl.id)}
                >
                  <td className="px-4 py-3 font-mono text-xs text-[var(--text-primary)]">{dl.step_id}</td>
                  <td className="px-4 py-3 text-sm text-[var(--text-primary)]">{dl.agent_id}</td>
                  <td className="px-4 py-3 max-w-[300px] text-sm text-[var(--text-muted)] truncate" title={dl.error}>
                    {dl.error}
                  </td>
                  <td className="px-4 py-3 text-sm text-[var(--text-secondary)]">{dl.attempt}/{dl.max_retries + 1}</td>
                  <td className="px-4 py-3 text-xs text-[var(--text-secondary)] whitespace-nowrap">
                    {formatTime(dl.created_at)}
                  </td>
                  <td className="px-4 py-3">
                    <span className={`inline-flex items-center px-2.5 py-0.5 rounded-full text-[10px] font-semibold capitalize border ${statusBadge(dl.status)}`}>
                      {dl.status}
                    </span>
                  </td>
                  <td className="px-4 py-3 text-right">
                    {dl.status === 'pending' && (
                      <div className="flex gap-2 justify-end" onClick={(e) => e.stopPropagation()}>
                        <button
                          onClick={() => handleRetry(dl.id)}
                          className="px-2.5 py-1 text-xs font-medium bg-[var(--glow-primary)] text-[var(--accent-primary)] rounded-lg hover:bg-[var(--accent-primary)] hover:text-white transition-all duration-300"
                        >
                          Retry
                        </button>
                        <button
                          onClick={() => handleDismiss(dl.id)}
                          className="px-2.5 py-1 text-xs font-medium text-[var(--text-muted)] hover:text-[var(--text-primary)] border border-[var(--bg-secondary)] rounded-lg hover:bg-[var(--glow-secondary)] transition-all duration-300"
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
          <div className="glass-card p-4 animate-fade-in">
            <h3 className="text-sm font-semibold mb-3 text-[var(--text-primary)]">Dead Letter Details</h3>
            <div className="grid grid-cols-2 gap-2 text-xs mb-3">
              <div><span className="text-[var(--text-muted)]">ID:</span> <span className="text-[var(--text-secondary)] font-mono">{dl.id}</span></div>
              <div><span className="text-[var(--text-muted)]">Pipeline Run:</span> <span className="text-[var(--text-secondary)] font-mono">{dl.pipeline_run_id}</span></div>
              <div><span className="text-[var(--text-muted)]">Retried At:</span> <span className="text-[var(--text-secondary)]">{dl.retried_at ? formatTime(dl.retried_at) : '-'}</span></div>
              <div><span className="text-[var(--text-muted)]">Dismissed By:</span> <span className="text-[var(--text-secondary)]">{dl.dismissed_by || '-'}</span></div>
            </div>
            <div className="text-xs">
              <span className="text-[var(--text-muted)]">Input:</span>
              <pre className="mt-1 bg-[var(--bg-primary)] rounded-lg p-3 overflow-x-auto max-h-40 text-[var(--text-secondary)] font-mono">
                {JSON.stringify(dl.input, null, 2)}
              </pre>
            </div>
            <div className="text-xs mt-2">
              <span className="text-[var(--text-muted)]">Error:</span>
              <pre className="mt-1 bg-[#ff446610] rounded-lg p-3 overflow-x-auto max-h-40 text-[#ff6680] font-mono">
                {dl.error}
              </pre>
            </div>
          </div>
        );
      })()}
    </div>
  );
}
