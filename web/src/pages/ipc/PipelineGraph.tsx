import { useState, useCallback, useEffect, useRef } from 'react';
import { apiFetch } from '@/lib/api';

export default function PipelineGraph() {
  const [pipelines, setPipelines] = useState<string[]>([]);
  const [selected, setSelected] = useState('');
  const [graph, setGraph] = useState('');
  const [format, setFormat] = useState<'mermaid' | 'ascii'>('mermaid');
  const [loading, setLoading] = useState(false);
  const [listLoaded, setListLoaded] = useState(false);
  const [renderError, setRenderError] = useState(false);
  const mermaidRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    (async () => {
      try {
        const data = await apiFetch<{ pipelines: string[] }>('/api/pipelines/list');
        const names: string[] = data.pipelines || [];
        setPipelines(names);
        if (names.length > 0 && !selected && names[0] !== undefined) {
          setSelected(names[0]);
        }
      } catch {
        // pipeline engine may be disabled
      } finally {
        setListLoaded(true);
      }
    })();
  }, []);

  const loadGraph = useCallback(async () => {
    if (!selected) return;
    setLoading(true);
    setRenderError(false);
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

  useEffect(() => {
    if (selected) loadGraph();
  }, [selected, format, loadGraph]);

  // Render mermaid diagram
  useEffect(() => {
    if (format !== 'mermaid' || !graph || !mermaidRef.current) return;
    const el = mermaidRef.current;
    const win = window as unknown as { __mermaid?: { render: (id: string, code: string) => Promise<{ svg: string }> } };

    if (win.__mermaid) {
      const renderDiv = document.createElement('div');
      document.body.appendChild(renderDiv);
      win.__mermaid.render('pipeline-graph-' + Date.now(), graph).then(({ svg }) => {
        el.innerHTML = svg;
        setRenderError(false);
      }).catch(() => {
        setRenderError(true);
      }).finally(() => {
        renderDiv.remove();
      });
    } else {
      setRenderError(true);
    }
  }, [graph, format]);

  return (
    <div className="p-6 space-y-6 animate-fade-in">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-xl font-bold text-gradient">Pipeline Graph</h1>
          <p className="text-xs text-[var(--text-secondary)] mt-1">Visualize pipeline step flow and transitions</p>
        </div>
        <div className="flex items-center gap-2">
          <select
            value={selected}
            onChange={(e) => setSelected(e.target.value)}
            className="input-warm px-3 py-2 text-sm"
          >
            {pipelines.length === 0 && (
              <option value="">No pipelines</option>
            )}
            {pipelines.map((name) => (
              <option key={name} value={name}>{name}</option>
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
          <button
            onClick={loadGraph}
            disabled={loading || !selected}
            className="px-3 py-1.5 text-sm bg-[var(--glow-primary)] hover:bg-[var(--glow-primary)] text-[var(--accent-primary)] rounded-lg transition-colors"
          >
            {loading ? 'Loading...' : 'Refresh'}
          </button>
        </div>
      </div>

      {loading && (
        <div className="flex items-center justify-center h-32">
          <div className="h-8 w-8 border-2 border-[var(--glow-primary)] border-t-[var(--accent-primary)] rounded-full animate-spin" />
        </div>
      )}

      {/* ASCII graph */}
      {!loading && graph && format === 'ascii' && (
        <div className="glass-card p-6 overflow-x-auto">
          <pre className="font-mono text-sm text-[var(--text-primary)] whitespace-pre leading-relaxed">
            {graph}
          </pre>
        </div>
      )}

      {/* Mermaid rendered diagram */}
      {!loading && graph && format === 'mermaid' && (
        <div className="space-y-4">
          <div
            ref={mermaidRef}
            className="glass-card p-6 flex justify-center min-h-[200px] overflow-x-auto"
          />
          {renderError && (
            <div className="glass-card p-6 overflow-x-auto">
              <p className="text-xs text-[var(--text-muted)] mb-2">Mermaid render failed — showing source:</p>
              <pre className="font-mono text-sm text-[var(--text-primary)] whitespace-pre leading-relaxed">
                {graph}
              </pre>
            </div>
          )}
          {!renderError && (
            <details className="text-sm">
              <summary className="text-[var(--text-muted)] cursor-pointer hover:text-[var(--text-primary)] transition-colors">
                View Mermaid source
              </summary>
              <div className="glass-card mt-2 p-4 overflow-x-auto">
                <pre className="font-mono text-xs text-[var(--text-secondary)] whitespace-pre">
                  {graph}
                </pre>
              </div>
            </details>
          )}
        </div>
      )}

      {!loading && !graph && listLoaded && pipelines.length === 0 && (
        <div className="glass-card p-8 text-center">
          <p className="text-[var(--text-secondary)]">
            Pipeline engine is not enabled. Configure <code className="text-[var(--accent-primary)] font-mono text-xs">[pipelines]</code> in config.toml.
          </p>
        </div>
      )}

      {!loading && !graph && listLoaded && pipelines.length > 0 && (
        <div className="glass-card p-8 text-center">
          <p className="text-[var(--text-secondary)]">Select a pipeline to view its graph.</p>
        </div>
      )}
    </div>
  );
}
