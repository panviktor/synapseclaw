import { useState, useCallback, useEffect } from 'react';
import { useSearchParams } from 'react-router-dom';
import { t } from '@/lib/i18n';
import { fetchFleet, fetchAgentCron, deleteAgentCronJob, addAgentCronJob, fetchAgentCronRuns } from '@/lib/ipc-api';
import type { IpcAgent, CronJob, CronRun } from '@/types/ipc';
import AgentLink from '@/components/ipc/AgentLink';
import ConfirmDialog from '@/components/ipc/ConfirmDialog';

interface AgentCronJob extends CronJob {
  agent_id: string;
}

export default function FleetCron() {
  const [searchParams] = useSearchParams();
  const [agents, setAgents] = useState<IpcAgent[]>([]);
  const [jobs, setJobs] = useState<AgentCronJob[]>([]);
  const [loading, setLoading] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [errors, setErrors] = useState<string[]>([]);
  const [expandedJobKey, setExpandedJobKey] = useState<string | null>(null);
  const [runs, setRuns] = useState<CronRun[]>([]);
  const [runsLoading, setRunsLoading] = useState(false);
  const [pendingDelete, setPendingDelete] = useState<{ agent_id: string; job_id: string } | null>(null);
  const [showAdd, setShowAdd] = useState(false);
  const [addAgent, setAddAgent] = useState('');
  const [addSchedule, setAddSchedule] = useState('');
  const [addCommand, setAddCommand] = useState('');
  const [addName, setAddName] = useState('');
  const [addError, setAddError] = useState('');

  const filterAgent = searchParams.get('agent') ?? '';

  const loadAll = useCallback(async () => {
    setLoading(true);
    setErrors([]);
    try {
      const fleet = await fetchFleet();
      setAgents(fleet);

      const onlineAgents = fleet.filter((a) => a.status === 'online');
      const agentsToQuery = filterAgent
        ? onlineAgents.filter((a) => a.agent_id === filterAgent)
        : onlineAgents;

      const results = await Promise.allSettled(
        agentsToQuery.map(async (agent) => {
          const cronJobs = await fetchAgentCron(agent.agent_id);
          return cronJobs.map((job) => ({ ...job, agent_id: agent.agent_id }));
        }),
      );

      const allJobs: AgentCronJob[] = [];
      const errs: string[] = [];
      results.forEach((r, i) => {
        if (r.status === 'fulfilled') {
          allJobs.push(...r.value);
        } else {
          const agent = agentsToQuery[i];
          if (agent) errs.push(`${agent.agent_id}: unreachable`);
        }
      });

      setJobs(allJobs);
      setErrors(errs);
      setLoaded(true);
    } catch (err) {
      console.error('Fleet cron load failed:', err);
    } finally {
      setLoading(false);
    }
  }, [filterAgent]);

  useEffect(() => {
    loadAll();
  }, [loadAll]);

  const handleExpandRuns = async (agentId: string, jobId: string) => {
    const key = `${agentId}:${jobId}`;
    if (expandedJobKey === key) {
      setExpandedJobKey(null);
      return;
    }
    setExpandedJobKey(key);
    setRunsLoading(true);
    try {
      const data = await fetchAgentCronRuns(agentId, jobId, 10);
      setRuns(data);
    } catch {
      setRuns([]);
    } finally {
      setRunsLoading(false);
    }
  };

  const handleDelete = async () => {
    if (!pendingDelete) return;
    try {
      await deleteAgentCronJob(pendingDelete.agent_id, pendingDelete.job_id);
      setPendingDelete(null);
      loadAll();
    } catch (err) {
      console.error('Delete failed:', err);
      setPendingDelete(null);
    }
  };

  const selectedAgentOnline = agents.find((a) => a.agent_id === addAgent)?.status === 'online';

  const handleAdd = async () => {
    setAddError('');
    if (!addAgent || !addSchedule || !addCommand) {
      setAddError('Agent, schedule, and command are required');
      return;
    }
    if (!selectedAgentOnline) {
      setAddError('Cannot add job: selected agent is offline');
      return;
    }
    try {
      await addAgentCronJob(addAgent, {
        name: addName || undefined,
        schedule: addSchedule,
        command: addCommand,
      });
      setShowAdd(false);
      setAddAgent('');
      setAddSchedule('');
      setAddCommand('');
      setAddName('');
      loadAll();
    } catch (err) {
      setAddError(err instanceof Error ? err.message : 'Failed to add job');
    }
  };

  return (
    <div className="p-6 space-y-6 animate-fade-in">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-xl font-bold text-gradient">{t('nav.ipc_cron') || 'Fleet Cron'}</h1>
          <p className="text-xs text-[var(--text-secondary)] mt-1">{t('ipc.cron_subtitle')}</p>
        </div>
        <div className="flex items-center gap-2">
          <button
            onClick={() => setShowAdd(true)}
            className="px-3 py-1.5 text-sm bg-[var(--glow-primary)] hover:bg-[var(--glow-primary)] text-[var(--accent-primary)] rounded-lg transition-colors"
          >
            + Add Job
          </button>
          <button
            onClick={loadAll}
            disabled={loading}
            className="px-3 py-1.5 text-sm bg-[var(--glow-primary)] hover:bg-[var(--glow-primary)] text-[var(--accent-primary)] rounded-lg transition-colors"
          >
            {loading ? 'Loading...' : 'Refresh'}
          </button>
        </div>
      </div>

      {errors.length > 0 && (
        <div className="bg-yellow-500/10 border border-yellow-500/30 rounded-xl p-3 text-sm text-yellow-400">
          {errors.map((e, i) => <div key={i}>{e}</div>)}
        </div>
      )}

      {/* Add job dialog */}
      {showAdd && (
        <div className="glass-card p-4 space-y-3">
          <h3 className="text-sm font-semibold text-[var(--text-primary)]">Add Cron Job</h3>
          <div className="grid grid-cols-2 gap-3">
            <select
              value={addAgent}
              onChange={(e) => setAddAgent(e.target.value)}
              className="input-warm px-3 py-2 text-sm"
            >
              <option value="">Select agent...</option>
              {agents.map((a) => (
                <option key={a.agent_id} value={a.agent_id}>
                  {a.agent_id}{a.status !== 'online' ? ` (${a.status})` : ''}
                </option>
              ))}
            </select>
            <input
              placeholder="Name (optional)"
              value={addName}
              onChange={(e) => setAddName(e.target.value)}
              className="input-warm px-3 py-2 text-sm"
            />
            <input
              placeholder="Schedule (e.g. */5 * * * *)"
              value={addSchedule}
              onChange={(e) => setAddSchedule(e.target.value)}
              className="input-warm px-3 py-2 text-sm"
            />
            <input
              placeholder="Command"
              value={addCommand}
              onChange={(e) => setAddCommand(e.target.value)}
              className="input-warm px-3 py-2 text-sm"
            />
          </div>
          {addError && <p className="text-red-400 text-xs">{addError}</p>}
          <div className="flex gap-2 justify-end">
            <button
              onClick={() => setShowAdd(false)}
              className="px-3 py-1.5 text-sm text-[var(--text-secondary)] hover:text-[var(--text-primary)] transition-colors"
            >
              Cancel
            </button>
            <button
              onClick={handleAdd}
              className="px-3 py-1.5 text-sm bg-[var(--accent-primary)] hover:bg-[var(--accent-primary)] text-white rounded-lg transition-colors"
            >
              Create
            </button>
          </div>
        </div>
      )}

      {/* Jobs table */}
      {!loaded && loading && (
        <div className="text-center py-12 text-[var(--text-secondary)]">Loading cron jobs...</div>
      )}

      {loaded && jobs.length === 0 && (
        <div className="text-center py-12 text-[var(--text-secondary)]">No cron jobs found</div>
      )}

      {loaded && jobs.length > 0 && (
        <div className="glass-card overflow-hidden">
          <table className="w-full text-sm">
            <thead>
              <tr className="text-left text-[var(--text-secondary)] border-b border-[var(--bg-secondary)]">
                <th className="px-4 py-3 w-[120px]">Agent</th>
                <th className="px-4 py-3 w-[140px]">Name</th>
                <th className="px-4 py-3">Command</th>
                <th className="px-4 py-3 w-[160px]">Next Run</th>
                <th className="px-4 py-3 w-[160px]">Last Run</th>
                <th className="px-4 py-3 w-[80px]">Status</th>
                <th className="px-4 py-3 w-[80px]">Actions</th>
              </tr>
            </thead>
            <tbody>
              {jobs.map((job) => {
                const jobKey = `${job.agent_id}:${job.id}`;
                const isExpanded = expandedJobKey === jobKey;
                return (
                  <>
                    <tr
                      key={jobKey}
                      className="border-b border-[var(--bg-secondary)] hover:bg-[var(--glow-secondary)] transition-colors cursor-pointer"
                      onClick={() => handleExpandRuns(job.agent_id, job.id)}
                    >
                      <td className="px-4 py-2.5">
                        <AgentLink agentId={job.agent_id} />
                      </td>
                      <td className="px-4 py-2.5 text-[var(--text-primary)] font-medium">
                        {job.name || job.id}
                      </td>
                      <td className="px-4 py-2.5 text-[var(--text-muted)] font-mono text-xs max-w-[300px] truncate" title={job.command}>
                        {job.command}
                      </td>
                      <td className="px-4 py-2.5 text-[var(--text-muted)] text-xs">
                        {job.next_run ? new Date(job.next_run).toLocaleString() : '—'}
                      </td>
                      <td className="px-4 py-2.5 text-[var(--text-muted)] text-xs">
                        {job.last_run ? new Date(job.last_run).toLocaleString() : '—'}
                      </td>
                      <td className="px-4 py-2.5">
                        <span className={`inline-block px-2 py-0.5 rounded-full text-xs font-medium ${
                          job.enabled ? 'bg-green-500/20 text-green-400' : 'bg-gray-500/20 text-gray-400'
                        }`}>
                          {job.enabled ? 'active' : 'disabled'}
                        </span>
                      </td>
                      <td className="px-4 py-2.5">
                        <button
                          onClick={(e) => {
                            e.stopPropagation();
                            setPendingDelete({ agent_id: job.agent_id, job_id: job.id });
                          }}
                          className="px-2 py-1 text-xs text-red-400 hover:text-red-300 hover:bg-red-500/10 rounded-lg transition-colors"
                        >
                          Delete
                        </button>
                      </td>
                    </tr>
                    {isExpanded && (
                      <tr key={`${jobKey}-runs`} className="bg-[var(--bg-primary)]">
                        <td colSpan={7} className="px-6 py-3">
                          {runsLoading ? (
                            <div className="text-[var(--text-secondary)] text-xs">Loading runs...</div>
                          ) : runs.length === 0 ? (
                            <div className="text-[var(--text-secondary)] text-xs">No recent runs</div>
                          ) : (
                            <table className="w-full text-xs">
                              <thead>
                                <tr className="text-[var(--text-secondary)]">
                                  <th className="text-left py-1 pr-4">Started</th>
                                  <th className="text-left py-1 pr-4">Finished</th>
                                  <th className="text-left py-1 pr-4">Status</th>
                                  <th className="text-left py-1 pr-4">Duration</th>
                                  <th className="text-left py-1">Output</th>
                                </tr>
                              </thead>
                              <tbody>
                                {runs.map((run) => (
                                  <tr key={run.id} className="text-[var(--text-muted)]">
                                    <td className="py-1 pr-4">{new Date(run.started_at).toLocaleString()}</td>
                                    <td className="py-1 pr-4">{new Date(run.finished_at).toLocaleString()}</td>
                                    <td className="py-1 pr-4">
                                      <span className={run.status === 'success' ? 'text-green-400' : 'text-red-400'}>
                                        {run.status}
                                      </span>
                                    </td>
                                    <td className="py-1 pr-4">{run.duration_ms ?? 0}ms</td>
                                    <td className="py-1 max-w-[300px] truncate font-mono" title={run.output ?? ''}>
                                      {run.output ?? '—'}
                                    </td>
                                  </tr>
                                ))}
                              </tbody>
                            </table>
                          )}
                        </td>
                      </tr>
                    )}
                  </>
                );
              })}
            </tbody>
          </table>
        </div>
      )}

      <div className="text-xs text-[var(--text-secondary)] text-center">
        {jobs.length > 0 && `${jobs.length} job${jobs.length !== 1 ? 's' : ''} across fleet`}
      </div>

      {pendingDelete && (
        <ConfirmDialog
          open={true}
          title="Delete Cron Job"
          message={`Delete job ${pendingDelete.job_id} on agent ${pendingDelete.agent_id}?`}
          onConfirm={handleDelete}
          onCancel={() => setPendingDelete(null)}
        />
      )}
    </div>
  );
}
