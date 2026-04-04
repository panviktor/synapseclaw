import { useState, useCallback, useEffect, useMemo } from 'react';
import { useSearchParams, useNavigate } from 'react-router-dom';
import { Activity as ActivityIcon, ArrowRight, Radio, Sparkles } from 'lucide-react';
import { fetchActivity, fetchFleet } from '@/lib/ipc-api';
import type { ActivityEvent, ActivityFilter, IpcAgent } from '@/types/ipc';
import AgentLink from '@/components/ipc/AgentLink';
import { TimeAbsolute } from '@/components/ipc/TimeAgo';
import { TIME_RANGES, timeRangeToTs } from '@/components/ipc/TimeRangeFilter';

const EVENT_TYPES = ['', 'ipc_send', 'spawn_start', 'spawn_complete', 'chat_message', 'channel_message', 'cron_run'];
const SURFACES = ['', 'ipc', 'spawn', 'web_chat', 'channel', 'cron'];
const PAGE_SIZE = 100;
const REFRESH_INTERVAL = 30_000;

function surfaceLabel(surface: string): string {
  switch (surface) {
    case 'ipc': return 'IPC';
    case 'spawn': return 'Spawn';
    case 'web_chat': return 'Web Chat';
    case 'channel': return 'Channel';
    case 'cron': return 'Cron';
    default: return surface;
  }
}

function surfaceColor(surface: string): string {
  switch (surface) {
    case 'ipc': return 'bg-blue-500/20 text-blue-400';
    case 'spawn': return 'bg-purple-500/20 text-purple-400';
    case 'web_chat': return 'bg-green-500/20 text-green-400';
    case 'channel': return 'bg-yellow-500/20 text-yellow-400';
    case 'cron': return 'bg-orange-500/20 text-orange-400';
    default: return 'bg-gray-500/20 text-gray-400';
  }
}

function traceUrl(event: ActivityEvent): string | null {
  const r = event.trace_ref;
  const agentId = encodeURIComponent(event.agent_id);
  switch (r.surface) {
    case 'ipc':
      if (r.session_id) return `/ipc/sessions?session_id=${encodeURIComponent(r.session_id)}`;
      if (r.from_agent) return `/ipc/sessions?agent_id=${encodeURIComponent(r.from_agent)}`;
      return '/ipc/sessions';
    case 'spawn':
      if (r.spawn_run_id) return `/ipc/spawns?session_id=${encodeURIComponent(r.spawn_run_id)}`;
      if (r.parent_agent_id) return `/ipc/spawns?parent_id=${encodeURIComponent(r.parent_agent_id)}`;
      return '/ipc/spawns';
    case 'web_chat':
      if (r.chat_session_key) return `/agents?agent=${agentId}&session=${encodeURIComponent(r.chat_session_key)}`;
      return `/agents?agent=${agentId}`;
    case 'channel':
      if (r.channel_session_key) return `/ipc/conversation?agent=${agentId}&key=${encodeURIComponent(r.channel_session_key)}`;
      return `/ipc/fleet/${agentId}`;
    case 'cron':
      return `/ipc/cron?agent=${agentId}`;
    default:
      return null;
  }
}

function traceLabel(surface: string): string {
  switch (surface) {
    case 'ipc': return 'Open IPC Session';
    case 'spawn': return 'Open Spawn Run';
    case 'web_chat': return 'Open Chat Session';
    case 'channel': return 'Open Conversation';
    case 'cron': return 'Open Cron';
    default: return 'Open';
  }
}

function ActivityStatCard({
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
      <p className="text-[10px] uppercase tracking-[0.22em] text-[var(--text-placeholder)]">{label}</p>
      <p className="mt-2 text-2xl font-semibold tracking-tight text-[var(--text-primary)]">{value}</p>
      <p className="mt-1 text-xs text-[var(--text-muted)]">{caption}</p>
    </div>
  );
}

export default function Activity() {
  const [searchParams, setSearchParams] = useSearchParams();
  const navigate = useNavigate();
  const [events, setEvents] = useState<ActivityEvent[]>([]);
  const [agents, setAgents] = useState<IpcAgent[]>([]);
  const [loading, setLoading] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [partial, setPartial] = useState(false);
  const [timeRange, setTimeRange] = useState('1h');

  const agentId = searchParams.get('agent_id') ?? '';
  const eventType = searchParams.get('event_type') ?? '';
  const surface = searchParams.get('surface') ?? '';

  const doSearch = useCallback(async () => {
    setLoading(true);
    try {
      const filters: ActivityFilter = { limit: PAGE_SIZE };
      if (agentId) filters.agent_id = agentId;
      if (eventType) filters.event_type = eventType;
      if (surface) filters.surface = surface;
      const fromTs = timeRangeToTs(timeRange);
      if (fromTs) filters.from_ts = fromTs;
      const data = await fetchActivity(filters);
      setEvents(data.events);
      setPartial(data.partial);
      setLoaded(true);
    } catch (err) {
      console.error('Activity fetch failed:', err);
    } finally {
      setLoading(false);
    }
  }, [agentId, eventType, surface, timeRange]);

  useEffect(() => {
    fetchFleet().then(setAgents).catch(() => {});
  }, []);

  useEffect(() => {
    doSearch();
    const timer = setInterval(doSearch, REFRESH_INTERVAL);
    return () => clearInterval(timer);
  }, [doSearch]);

  const updateParam = (key: string, value: string) => {
    setSearchParams((prev) => {
      const next = new URLSearchParams(prev);
      if (value) next.set(key, value);
      else next.delete(key);
      return next;
    });
  };

  const uniqueAgents = useMemo(() => new Set(events.map((event) => event.agent_id)).size, [events]);
  const dominantSurface = useMemo(() => {
    const counts = events.reduce<Record<string, number>>((acc, event) => {
      const key = event.trace_ref.surface;
      acc[key] = (acc[key] ?? 0) + 1;
      return acc;
    }, {});
    return Object.entries(counts).sort((a, b) => b[1] - a[1])[0]?.[0] ?? 'none';
  }, [events]);

  return (
    <div className="p-6 space-y-6 animate-fade-in">
      <div className="relative overflow-hidden rounded-[28px] border border-[var(--border-default)] bg-[linear-gradient(135deg,var(--glow-primary),transparent_35%),var(--bg-card)] px-6 py-6">
        <div className="absolute inset-x-10 top-0 h-px bg-gradient-to-r from-transparent via-[var(--accent-primary)]/70 to-transparent" />
        <div className="flex flex-col gap-5 xl:flex-row xl:items-end xl:justify-between">
          <div>
            <p className="text-[11px] font-semibold uppercase tracking-[0.28em] text-[var(--text-placeholder)]">
              Activity Scope
            </p>
            <div className="mt-2 flex items-center gap-2">
              <ActivityIcon className="h-6 w-6 text-[var(--accent-primary)]" />
              <h1 className="text-3xl font-semibold tracking-tight text-[var(--text-primary)]">
                Cross-Surface Activity Feed
              </h1>
            </div>
            <p className="mt-2 max-w-2xl text-sm text-[var(--text-muted)]">
              Watch IPC, spawn, web chat, channel, and cron events as one operator-visible stream.
            </p>
          </div>
          <div className="flex items-center gap-2">
            {partial && (
              <span className="rounded-full bg-yellow-500/10 px-2.5 py-1 text-xs font-semibold uppercase tracking-wide text-yellow-400">
                partial
              </span>
            )}
            <button
              onClick={doSearch}
              disabled={loading}
              className="btn-secondary px-4 py-2 text-sm"
            >
              {loading ? 'Loading...' : 'Refresh'}
            </button>
          </div>
        </div>
      </div>

      <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-4">
        <ActivityStatCard label="Events" value={`${events.length}`} caption="visible in current filter window" />
        <ActivityStatCard label="Agents" value={`${uniqueAgents}`} caption="participating in this slice" />
        <ActivityStatCard label="Dominant Surface" value={surfaceLabel(dominantSurface)} caption="highest event share right now" />
        <ActivityStatCard label="Window" value={timeRange} caption="active time range filter" />
      </div>

      <div className="glass-card overflow-hidden">
        <div className="border-b border-[var(--bg-secondary)] px-5 py-4">
          <div className="flex items-center gap-2">
            <Radio className="h-4 w-4 text-[var(--accent-primary)]" />
            <h2 className="text-sm font-semibold uppercase tracking-wider text-[var(--text-primary)]">Filters</h2>
          </div>
        </div>
        <div className="flex flex-wrap gap-3 px-5 py-5">
          <select
            value={agentId}
            onChange={(e) => updateParam('agent_id', e.target.value)}
            className="input-warm min-w-[160px] px-3 py-2 text-sm"
          >
            <option value="">All agents</option>
            {agents.map((a) => (
              <option key={a.agent_id} value={a.agent_id}>{a.agent_id}</option>
            ))}
          </select>

          <select
            value={eventType}
            onChange={(e) => updateParam('event_type', e.target.value)}
            className="input-warm px-3 py-2 text-sm"
          >
            {EVENT_TYPES.map((et) => (
              <option key={et} value={et}>{et || 'All types'}</option>
            ))}
          </select>

          <select
            value={surface}
            onChange={(e) => updateParam('surface', e.target.value)}
            className="input-warm px-3 py-2 text-sm"
          >
            {SURFACES.map((s) => (
              <option key={s} value={s}>{s ? surfaceLabel(s) : 'All surfaces'}</option>
            ))}
          </select>

          <select
            value={timeRange}
            onChange={(e) => setTimeRange(e.target.value)}
            className="input-warm px-3 py-2 text-sm"
          >
            {TIME_RANGES.map((r) => (
              <option key={r.value} value={r.value}>{r.label}</option>
            ))}
          </select>
        </div>
      </div>

      {!loaded && loading && (
        <div className="py-12 text-center text-[var(--text-secondary)]">Loading activity...</div>
      )}

      {loaded && events.length === 0 && (
        <div className="glass-card px-6 py-12 text-center">
          <Sparkles className="mx-auto mb-3 h-12 w-12 text-[var(--accent-primary)]/40" />
          <p className="text-lg font-semibold text-[var(--text-primary)]">No activity events found</p>
          <p className="mt-2 text-sm text-[var(--text-muted)]">Try widening the time window or clearing filters.</p>
        </div>
      )}

      {loaded && events.length > 0 && (
        <div className="glass-card overflow-hidden">
          <div className="border-b border-[var(--bg-secondary)] px-5 py-4">
            <div className="flex items-center gap-2">
              <ActivityIcon className="h-4 w-4 text-[var(--accent-primary)]" />
              <h2 className="text-sm font-semibold uppercase tracking-wider text-[var(--text-primary)]">Event Stream</h2>
            </div>
          </div>
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-[var(--bg-secondary)] text-left text-[var(--text-secondary)] text-xs uppercase tracking-wider">
                  <th className="w-[160px] px-4 py-3">Time</th>
                  <th className="w-[120px] px-4 py-3">Agent</th>
                  <th className="w-[100px] px-4 py-3">Surface</th>
                  <th className="w-[130px] px-4 py-3">Type</th>
                  <th className="px-4 py-3">Summary</th>
                  <th className="w-[120px] px-4 py-3 text-right">Trace</th>
                </tr>
              </thead>
              <tbody>
                {events.map((event, idx) => {
                  const url = traceUrl(event);
                  return (
                    <tr
                      key={`${event.event_type}-${event.agent_id}-${event.timestamp}-${idx}`}
                      className="border-b border-[var(--bg-secondary)] transition-colors hover:bg-[var(--glow-secondary)]"
                    >
                      <td className="whitespace-nowrap px-4 py-3 text-[var(--text-muted)]">
                        <TimeAbsolute timestamp={event.timestamp} />
                      </td>
                      <td className="px-4 py-3">
                        <AgentLink agentId={event.agent_id} />
                      </td>
                      <td className="px-4 py-3">
                        <span className={`inline-block rounded-full px-2 py-0.5 text-xs font-medium ${surfaceColor(event.trace_ref.surface)}`}>
                          {surfaceLabel(event.trace_ref.surface)}
                        </span>
                      </td>
                      <td className="px-4 py-3 font-mono text-xs text-[var(--text-muted)]">
                        {event.event_type}
                      </td>
                      <td className="max-w-[420px] px-4 py-3 text-[var(--text-muted)]">
                        <div className="line-clamp-2" title={event.summary}>{event.summary}</div>
                      </td>
                      <td className="px-4 py-3 text-right">
                        {url && (
                          <button
                            onClick={() => navigate(url)}
                            className="inline-flex items-center gap-1 rounded-lg bg-[var(--glow-primary)] px-2.5 py-1.5 text-xs text-[var(--accent-primary)] transition-colors hover:bg-[var(--glow-primary)]"
                            title={traceLabel(event.trace_ref.surface)}
                          >
                            Open
                            <ArrowRight className="h-3 w-3" />
                          </button>
                        )}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </div>
      )}

      <div className="text-center text-xs text-[var(--text-secondary)]">
        {events.length > 0 && `${events.length} event${events.length !== 1 ? 's' : ''}`}
        {partial && ' (partial results)'}
      </div>
    </div>
  );
}
