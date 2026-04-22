import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  Activity,
  Archive,
  BookMarked,
  CheckCircle2,
  FileText,
  RotateCcw,
  ShieldCheck,
  Sparkles,
  TestTube2,
} from 'lucide-react';
import { apiFetch } from '@/lib/api';

type SkillStatus = 'active' | 'candidate' | 'deprecated' | string;

interface LearnedSkill {
  id: string;
  name: string;
  description?: string;
  status?: SkillStatus;
  origin?: string;
  version?: number;
  task_family?: string | null;
  tool_pattern?: string[];
  tags?: string[];
  success_count?: number;
  fail_count?: number;
}

interface PatchCandidate {
  id: string;
  target_skill_id?: string;
  target_version?: number;
  status?: string;
  diff_summary?: string;
  replay_criteria?: string[];
  eval_results?: Array<{ criterion: string; status: string; evidence?: string | null }>;
}

interface SkillHealthItem {
  skill_id: string;
  name: string;
  status: SkillStatus;
  severity: string;
  signals?: string[];
  utility?: Record<string, number>;
}

interface SkillUseTrace {
  skill_id?: string;
  skill_name?: string;
  outcome?: string;
  observed_at_unix?: number;
  tool_roles?: string[];
}

interface CandidatesResponse {
  skills: LearnedSkill[];
  patch_candidates: PatchCandidate[];
}

interface AuthoredResponse {
  skills: LearnedSkill[];
}

interface TracesResponse {
  traces: SkillUseTrace[];
}

interface HealthResponse {
  report?: { items?: SkillHealthItem[] };
  cleanup_decisions?: unknown[];
}

type Tab = 'skills' | 'candidates' | 'health' | 'authoring';

const tabs: Array<{ id: Tab; label: string }> = [
  { id: 'skills', label: 'Skills' },
  { id: 'candidates', label: 'Candidates' },
  { id: 'health', label: 'Health' },
  { id: 'authoring', label: 'Authoring' },
];

function asError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function compactJson(value: unknown): string {
  if (value === undefined || value === null) return '';
  if (typeof value === 'string') return value;
  return JSON.stringify(value, null, 2);
}

function badgeTone(value?: string): string {
  switch ((value ?? '').toLowerCase()) {
    case 'active':
    case 'passed':
    case 'healthy':
      return 'border-[var(--status-success)] text-[var(--status-success)]';
    case 'candidate':
    case 'watch':
      return 'border-[var(--status-warning)] text-[var(--status-warning)]';
    case 'deprecated':
    case 'failed':
    case 'review':
      return 'border-[var(--status-error)] text-[var(--status-error)]';
    default:
      return 'border-theme-default text-theme-muted';
  }
}

function StatusBadge({ value }: { value?: string }) {
  return (
    <span className={`rounded-md border px-2 py-0.5 text-xs ${badgeTone(value)}`}>
      {value || 'unknown'}
    </span>
  );
}

function Panel({
  title,
  icon: Icon,
  children,
}: {
  title: string;
  icon: typeof BookMarked;
  children: React.ReactNode;
}) {
  return (
    <section className="rounded-lg border border-theme-default bg-theme-card p-4">
      <div className="mb-4 flex items-center gap-2">
        <Icon className="h-4 w-4 text-theme-accent" />
        <h2 className="text-sm font-semibold uppercase tracking-wide text-theme-primary">{title}</h2>
      </div>
      {children}
    </section>
  );
}

export default function Skills() {
  const [tab, setTab] = useState<Tab>('skills');
  const [authored, setAuthored] = useState<LearnedSkill[]>([]);
  const [learnedCandidates, setLearnedCandidates] = useState<LearnedSkill[]>([]);
  const [patchCandidates, setPatchCandidates] = useState<PatchCandidate[]>([]);
  const [healthItems, setHealthItems] = useState<SkillHealthItem[]>([]);
  const [cleanupCount, setCleanupCount] = useState(0);
  const [traces, setTraces] = useState<SkillUseTrace[]>([]);
  const [selectedCandidate, setSelectedCandidate] = useState('');
  const [selectedSkill, setSelectedSkill] = useState('');
  const [output, setOutput] = useState('');
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState('');
  const [form, setForm] = useState({
    name: '',
    description: '',
    task_family: '',
    tools: '',
    tags: '',
    status: 'active',
    body: '# New skill\n\nDescribe the repeatable workflow.',
  });

  const refresh = useCallback(async () => {
    setLoading(true);
    setError('');
    try {
      const [authoredResponse, candidatesResponse, healthResponse, tracesResponse] =
        await Promise.all([
          apiFetch<AuthoredResponse>('/api/skills/authored?limit=100'),
          apiFetch<CandidatesResponse>('/api/skills/candidates?limit=100'),
          apiFetch<HealthResponse>('/api/skills/health?limit=100&trace_limit=100'),
          apiFetch<TracesResponse>('/api/skills/traces?limit=50'),
        ]);
      setAuthored(authoredResponse.skills ?? []);
      setLearnedCandidates(candidatesResponse.skills ?? []);
      setPatchCandidates(candidatesResponse.patch_candidates ?? []);
      setHealthItems(healthResponse.report?.items ?? []);
      setCleanupCount(healthResponse.cleanup_decisions?.length ?? 0);
      setTraces(tracesResponse.traces ?? []);
    } catch (err) {
      setError(asError(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const allSkills = useMemo(
    () => [...authored, ...learnedCandidates],
    [authored, learnedCandidates],
  );

  async function runAction(label: string, fn: () => Promise<unknown>, confirmText?: string) {
    if (confirmText && !window.confirm(confirmText)) return;
    setBusy(label);
    setError('');
    try {
      const result = await fn();
      setOutput(compactJson(result));
      await refresh();
    } catch (err) {
      setError(asError(err));
    } finally {
      setBusy('');
    }
  }

  const createBody = {
    name: form.name,
    description: form.description || null,
    body: form.body,
    task_family: form.task_family || null,
    tool_pattern: form.tools.split(',').map((item) => item.trim()).filter(Boolean),
    tags: form.tags.split(',').map((item) => item.trim()).filter(Boolean),
    status: form.status,
  };

  return (
    <div className="p-6 space-y-6 animate-fade-in">
      <div className="flex flex-col gap-4 lg:flex-row lg:items-end lg:justify-between">
        <div>
          <h1 className="text-2xl font-bold text-gradient">Skills</h1>
          <p className="mt-1 text-sm text-theme-muted">Governed activation, candidates, health and rollback.</p>
        </div>
        <button
          type="button"
          onClick={() => void refresh()}
          className="btn-secondary rounded-lg px-4 py-2 text-sm"
          disabled={loading || !!busy}
        >
          Refresh
        </button>
      </div>

      <div className="flex flex-wrap gap-2">
        {tabs.map((item) => (
          <button
            key={item.id}
            type="button"
            onClick={() => setTab(item.id)}
            className={`rounded-lg border px-3 py-2 text-sm transition-colors ${
              tab === item.id
                ? 'border-[var(--accent-primary)] bg-theme-secondary text-theme-primary'
                : 'border-theme-default text-theme-muted hover:bg-theme-hover'
            }`}
          >
            {item.label}
          </button>
        ))}
      </div>

      {error && (
        <div className="rounded-lg border border-[var(--status-error)] bg-theme-card p-3 text-sm text-[var(--status-error)]">
          {error}
        </div>
      )}
      {busy && <div className="text-sm text-theme-muted">Running {busy}...</div>}
      {loading ? (
        <div className="flex h-40 items-center justify-center">
          <div className="h-8 w-8 rounded-full border-2 border-theme-default border-t-[var(--accent-primary)] animate-spin" />
        </div>
      ) : (
        <>
          {tab === 'skills' && (
            <div className="grid gap-4 xl:grid-cols-[1fr_24rem]">
              <Panel title={`Memory-backed skills (${allSkills.length})`} icon={BookMarked}>
                <div className="space-y-3">
                  {allSkills.length === 0 && <p className="text-sm text-theme-muted">No memory-backed skills.</p>}
                  {allSkills.map((skill) => (
                    <button
                      key={skill.id}
                      type="button"
                      onClick={() => setSelectedSkill(skill.id)}
                      className="w-full rounded-lg border border-theme-default bg-theme-card-hover p-3 text-left hover:border-[var(--accent-primary)]"
                    >
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="font-medium text-theme-primary">{skill.name}</span>
                        <StatusBadge value={skill.status} />
                        <span className="text-xs text-theme-muted">v{skill.version ?? '?'}</span>
                        <span className="text-xs text-theme-muted">{skill.origin ?? 'learned'}</span>
                      </div>
                      <p className="mt-2 text-sm text-theme-muted">{skill.description || 'No description.'}</p>
                      <p className="mt-2 text-xs text-theme-placeholder">
                        {(skill.tool_pattern ?? []).join(', ') || 'no tool hints'}
                      </p>
                    </button>
                  ))}
                </div>
              </Panel>
              <Panel title="Selected skill actions" icon={FileText}>
                <input
                  value={selectedSkill}
                  onChange={(event) => setSelectedSkill(event.target.value)}
                  placeholder="Skill id or name"
                  className="input-warm mb-3 w-full rounded-lg px-3 py-2 text-sm"
                />
                <div className="grid gap-2">
                  <button
                    type="button"
                    className="btn-secondary rounded-lg px-3 py-2 text-sm"
                    onClick={() =>
                      void runAction('export', () =>
                        apiFetch('/api/skills/export', {
                          method: 'POST',
                          body: JSON.stringify({ skill: selectedSkill, overwrite: true }),
                        }),
                      'Export or overwrite workspace package for this skill?')
                    }
                  >
                    Export package
                  </button>
                  <button
                    type="button"
                    className="btn-secondary rounded-lg px-3 py-2 text-sm"
                    onClick={() =>
                      void runAction('versions', () =>
                        apiFetch(`/api/skills/versions?skill=${encodeURIComponent(selectedSkill)}&limit=50`),
                      )
                    }
                  >
                    Versions
                  </button>
                </div>
              </Panel>
            </div>
          )}

          {tab === 'candidates' && (
            <div className="grid gap-4 xl:grid-cols-[1fr_24rem]">
              <Panel title={`Patch candidates (${patchCandidates.length})`} icon={Sparkles}>
                <div className="space-y-3">
                  {patchCandidates.length === 0 && <p className="text-sm text-theme-muted">No patch candidates.</p>}
                  {patchCandidates.map((candidate) => (
                    <button
                      key={candidate.id}
                      type="button"
                      onClick={() => setSelectedCandidate(candidate.id)}
                      className="w-full rounded-lg border border-theme-default bg-theme-card-hover p-3 text-left hover:border-[var(--accent-primary)]"
                    >
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="font-mono text-sm text-theme-primary">{candidate.id}</span>
                        <StatusBadge value={candidate.status} />
                        <span className="text-xs text-theme-muted">target v{candidate.target_version ?? '?'}</span>
                      </div>
                      <p className="mt-2 text-sm text-theme-muted">{candidate.diff_summary || 'No diff summary.'}</p>
                      <p className="mt-2 text-xs text-theme-placeholder">
                        {(candidate.replay_criteria ?? []).join(' · ') || 'no replay criteria'}
                      </p>
                    </button>
                  ))}
                </div>
              </Panel>
              <Panel title={`Generated skills (${learnedCandidates.length})`} icon={TestTube2}>
                <input
                  value={selectedCandidate}
                  onChange={(event) => setSelectedCandidate(event.target.value)}
                  placeholder="Patch candidate id"
                  className="input-warm mb-3 w-full rounded-lg px-3 py-2 text-sm"
                />
                <div className="grid gap-2">
                  <button type="button" className="btn-secondary rounded-lg px-3 py-2 text-sm" onClick={() => void runAction('diff', () => apiFetch('/api/skills/candidates/diff', { method: 'POST', body: JSON.stringify({ candidate: selectedCandidate }) }))}>Diff</button>
                  <button type="button" className="btn-secondary rounded-lg px-3 py-2 text-sm" onClick={() => void runAction('test', () => apiFetch('/api/skills/candidates/test', { method: 'POST', body: JSON.stringify({ candidate: selectedCandidate }) }))}>Test</button>
                  <button type="button" className="btn-primary rounded-lg px-3 py-2 text-sm" onClick={() => void runAction('apply', () => apiFetch('/api/skills/candidates/apply', { method: 'POST', body: JSON.stringify({ candidate: selectedCandidate }) }), 'Apply this tested patch candidate?')}>Apply</button>
                </div>
                <div className="mt-4 space-y-2">
                  {learnedCandidates.map((skill) => (
                    <div key={skill.id} className="rounded-lg border border-theme-default p-3">
                      <div className="flex items-center gap-2">
                        <span className="text-sm font-medium text-theme-primary">{skill.name}</span>
                        <StatusBadge value={skill.status} />
                      </div>
                      <p className="mt-1 text-xs text-theme-muted">{skill.description}</p>
                    </div>
                  ))}
                </div>
              </Panel>
            </div>
          )}

          {tab === 'health' && (
            <div className="grid gap-4 xl:grid-cols-[1fr_24rem]">
              <Panel title={`Health (${healthItems.length})`} icon={ShieldCheck}>
                <div className="space-y-3">
                  {healthItems.map((item) => (
                    <div key={item.skill_id} className="rounded-lg border border-theme-default bg-theme-card-hover p-3">
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="font-medium text-theme-primary">{item.name}</span>
                        <StatusBadge value={item.status} />
                        <StatusBadge value={item.severity} />
                      </div>
                      <p className="mt-2 text-xs text-theme-muted">
                        {(item.signals ?? []).join(', ') || 'no signals'} · {compactJson(item.utility)}
                      </p>
                    </div>
                  ))}
                </div>
              </Panel>
              <Panel title="Maintenance" icon={RotateCcw}>
                <p className="mb-3 text-sm text-theme-muted">{cleanupCount} cleanup decisions are available.</p>
                <div className="grid gap-2">
                  <button type="button" className="btn-secondary rounded-lg px-3 py-2 text-sm" onClick={() => void runAction('health apply', () => apiFetch('/api/skills/health/apply', { method: 'POST', body: JSON.stringify({}) }), 'Apply eligible skill health cleanup decisions?')}>Apply health cleanup</button>
                  <button type="button" className="btn-secondary rounded-lg px-3 py-2 text-sm" onClick={() => void runAction('autopromote dry-run', () => apiFetch('/api/skills/autopromote?limit=100'))}>Autopromote dry-run</button>
                  <button type="button" className="btn-primary rounded-lg px-3 py-2 text-sm" onClick={() => void runAction('autopromote apply', () => apiFetch('/api/skills/autopromote/apply', { method: 'POST', body: JSON.stringify({}) }), 'Apply eligible generated patch promotions?')}>Apply autopromote</button>
                </div>
                <div className="mt-4 space-y-2">
                  {traces.slice(0, 8).map((trace, index) => (
                    <div key={`${trace.skill_id ?? 'trace'}-${index}`} className="rounded-lg border border-theme-default p-2 text-xs text-theme-muted">
                      {trace.skill_name ?? trace.skill_id ?? 'skill'} · {trace.outcome ?? 'unknown'} · {(trace.tool_roles ?? []).join(', ')}
                    </div>
                  ))}
                </div>
              </Panel>
            </div>
          )}

          {tab === 'authoring' && (
            <div className="grid gap-4 xl:grid-cols-[1fr_24rem]">
              <Panel title="Create or update user skill" icon={CheckCircle2}>
                <div className="grid gap-3 md:grid-cols-2">
                  <input className="input-warm rounded-lg px-3 py-2 text-sm" placeholder="Name" value={form.name} onChange={(event) => setForm({ ...form, name: event.target.value })} />
                  <input className="input-warm rounded-lg px-3 py-2 text-sm" placeholder="Description" value={form.description} onChange={(event) => setForm({ ...form, description: event.target.value })} />
                  <input className="input-warm rounded-lg px-3 py-2 text-sm" placeholder="Task family" value={form.task_family} onChange={(event) => setForm({ ...form, task_family: event.target.value })} />
                  <input className="input-warm rounded-lg px-3 py-2 text-sm" placeholder="Tools, comma-separated" value={form.tools} onChange={(event) => setForm({ ...form, tools: event.target.value })} />
                  <input className="input-warm rounded-lg px-3 py-2 text-sm" placeholder="Tags, comma-separated" value={form.tags} onChange={(event) => setForm({ ...form, tags: event.target.value })} />
                  <select className="input-warm rounded-lg px-3 py-2 text-sm" value={form.status} onChange={(event) => setForm({ ...form, status: event.target.value })}>
                    <option value="active">active</option>
                    <option value="candidate">candidate</option>
                  </select>
                </div>
                <textarea className="input-warm mt-3 min-h-56 w-full rounded-lg px-3 py-2 font-mono text-sm" value={form.body} onChange={(event) => setForm({ ...form, body: event.target.value })} />
              </Panel>
              <Panel title="Authoring actions" icon={Archive}>
                <input
                  value={selectedSkill}
                  onChange={(event) => setSelectedSkill(event.target.value)}
                  placeholder="Skill id/name for update or rollback id"
                  className="input-warm mb-3 w-full rounded-lg px-3 py-2 text-sm"
                />
                <div className="grid gap-2">
                  <button type="button" className="btn-primary rounded-lg px-3 py-2 text-sm" onClick={() => void runAction('create skill', () => apiFetch('/api/skills/create', { method: 'POST', body: JSON.stringify(createBody) }))}>Create</button>
                  <button type="button" className="btn-secondary rounded-lg px-3 py-2 text-sm" onClick={() => void runAction('update skill', () => apiFetch('/api/skills/update', { method: 'POST', body: JSON.stringify({ skill: selectedSkill, ...createBody }) }), 'Update this memory-backed skill?')}>Update</button>
                  <button type="button" className="btn-secondary rounded-lg px-3 py-2 text-sm" onClick={() => void runAction('rollback', () => apiFetch('/api/skills/rollback', { method: 'POST', body: JSON.stringify({ rollback: selectedSkill }) }), 'Rollback using this apply record?')}>Rollback</button>
                </div>
              </Panel>
            </div>
          )}
        </>
      )}

      {output && (
        <Panel title="Last result" icon={Activity}>
          <pre className="max-h-96 overflow-auto rounded-lg bg-theme-secondary p-3 text-xs text-theme-muted">
            {output}
          </pre>
        </Panel>
      )}
    </div>
  );
}
