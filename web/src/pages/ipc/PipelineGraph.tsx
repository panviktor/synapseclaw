import { useState, useCallback, useEffect, useRef } from 'react';
import { apiFetch } from '@/lib/api';

interface PipelineInfo {
  name: string;
}

export default function PipelineGraph() {
  const [pipelines, setPipelines] = useState<PipelineInfo[]>([]);
  const [selected, setSelected] = useState('');
  const [graph, setGraph] = useState('');
  const [format, setFormat] = useState<'mermaid' | 'ascii'>('mermaid');
  const [loading, setLoading] = useState(false);
  const mermaidRef = useRef<HTMLDivElement>(null);

  // Load pipeline list
  useEffect(() => {
    (async () => {
      try {
        const data = await apiFetch<{ pipelines: string[] }>('/api/pipelines/list');
        const names: string[] = data.pipelines || [];
        setPipelines(names.map(n => ({ name: n })));
        if (names.length > 0 && !selected && names[0] !== undefined) {
          setSelected(names[0]);
        }
      } catch {
        // pipeline engine may be disabled
      }
    })();
  }, []);

  const loadGraph = useCallback(async () => {
    if (!selected) return;
    setLoading(true);
    try {
      const data = await apiFetch<{ graph: string }>(
        `/api/pipelines/${encodeURIComponent(selected)}/graph?format=${format}`
      );
      setGraph(data.graph || '');
    } catch {
      setGraph('Failed to load graph');
    } finally {
      setLoading(false);
    }
  }, [selected, format]);

  // Auto-load when selection changes
  useEffect(() => {
    if (selected) loadGraph();
  }, [selected, format, loadGraph]);

  // Render mermaid if available
  useEffect(() => {
    if (format !== 'mermaid' || !graph || !mermaidRef.current) return;
    const el = mermaidRef.current;

    // Try to use mermaid from CDN (loaded lazily)
    const win = window as unknown as { mermaid?: { render: (id: string, code: string) => Promise<{ svg: string }> } };
    if (win.mermaid) {
      win.mermaid.render('pipeline-graph', graph).then(({ svg }) => {
        el.innerHTML = svg;
      }).catch(() => {
        // fallback to code display
        el.innerHTML = '';
      });
    }
  }, [graph, format]);

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <h1 className="text-xl font-semibold text-theme-default">Pipeline Graph</h1>
        <div className="flex items-center gap-3">
          <select
            value={selected}
            onChange={(e) => setSelected(e.target.value)}
            className="input-warm px-3 py-2 text-sm"
          >
            {pipelines.length === 0 && (
              <option value="">No pipelines found</option>
            )}
            {pipelines.map((p) => (
              <option key={p.name} value={p.name}>{p.name}</option>
            ))}
          </select>
          <select
            value={format}
            onChange={(e) => setFormat(e.target.value as 'mermaid' | 'ascii')}
            className="input-warm px-3 py-2 text-sm"
          >
            <option value="mermaid">Mermaid</option>
            <option value="ascii">ASCII</option>
          </select>
        </div>
      </div>

      {loading && (
        <div className="text-center py-8 text-theme-muted">Loading...</div>
      )}

      {!loading && graph && format === 'ascii' && (
        <pre className="bg-theme-card border border-theme-default rounded-xl p-6 overflow-x-auto font-mono text-sm text-theme-default whitespace-pre">
          {graph}
        </pre>
      )}

      {!loading && graph && format === 'mermaid' && (
        <div className="space-y-4">
          <div
            ref={mermaidRef}
            className="bg-theme-card border border-theme-default rounded-xl p-6 flex justify-center min-h-[200px]"
          />
          <details className="text-sm">
            <summary className="text-theme-muted cursor-pointer hover:text-theme-default">
              View Mermaid source
            </summary>
            <pre className="mt-2 bg-theme-secondary rounded-lg p-4 overflow-x-auto font-mono text-xs text-theme-default">
              {graph}
            </pre>
          </details>
        </div>
      )}

      {!loading && !graph && pipelines.length === 0 && (
        <div className="text-center py-12 text-theme-muted">
          Pipeline engine is not enabled. Configure <code>[pipelines]</code> in config.toml.
        </div>
      )}
    </div>
  );
}
