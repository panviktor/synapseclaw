import { useState, useCallback, useEffect, useRef } from 'react';
import { useSearchParams, useNavigate } from 'react-router-dom';
import { t } from '@/lib/i18n';
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
      // Agent-scoped: pass agent= so AgentChat switches to the right agent
      if (r.chat_session_key) return `/agents?agent=${agentId}&session=${encodeURIComponent(r.chat_session_key)}`;
      return `/agents?agent=${agentId}`;
    case 'channel':
      // Channel sessions use a different key namespace — route to read-only viewer
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

export default function Activity() {
  const [searchParams, setSearchParams] = useSearchParams();
  const navigate = useNavigate();
  const [events, setEvents] = useState<ActivityEvent[]>([]);
  const [agents, setAgents] = useState<IpcAgent[]>([]);
  const [loading, setLoading] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [partial, setPartial] = useState(false);
  const [timeRange, setTimeRange] = useState('1h');
  const [expandedIdx, setExpandedIdx] = useState<number | null>(null);
  const userInputRef = useRef(false);

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

  // Load fleet for agent selector
  useEffect(() => {
    fetchFleet().then(setAgents).catch(() => {});
  }, []);

  // Initial load + auto-refresh
  useEffect(() => {
    doSearch();
    const timer = setInterval(doSearch, REFRESH_INTERVAL);
    return () => clearInterval(timer);
  }, [doSearch]);

  const updateParam = (key: string, value: string) => {
    userInputRef.current = true;
    setSearchParams((prev) => {
      const next = new URLSearchParams(prev);
      if (value) next.set(key, value);
      else next.delete(key);
      return next;
    });
  };

  return (
    <div className="p-6 space-y-6 animate-fade-in">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-xl font-bold text-gradient">{t('nav.ipc_activity') || 'Activity Feed'}</h1>
          <p className="text-xs text-[var(--text-secondary)] mt-1">{t('ipc.activity_subtitle')}</p>
        </div>
        <div className="flex items-center gap-2">
          {partial && (
            <span className="text-xs text-yellow-400 bg-yellow-500/10 px-2 py-1 rounded-lg">
              Partial — some agents unreachable
            </span>
          )}
          <button
            onClick={doSearch}
            disabled={loading}
            className="px-3 py-1.5 text-sm bg-[var(--glow-primary)] hover:bg-[var(--glow-primary)] text-[var(--accent-primary)] rounded-lg transition-colors"
          >
            {loading ? 'Loading...' : 'Refresh'}
          </button>
        </div>
      </div>

      {/* Filters */}
      <div className="flex flex-wrap gap-3">
        <select
          value={agentId}
          onChange={(e) => updateParam('agent_id', e.target.value)}
          className="input-warm px-3 py-2 text-sm min-w-[160px]"
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

      {/* Event list */}
      {!loaded && loading && (
        <div className="text-center py-12 text-[var(--text-secondary)]">Loading activity...</div>
      )}

      {loaded && events.length === 0 && (
        <div className="text-center py-12 text-[var(--text-secondary)]">No activity events found</div>
      )}

      {loaded && events.length > 0 && (
        <div className="glass-card overflow-hidden">
          <table className="w-full text-sm">
            <thead>
              <tr className="text-left text-[var(--text-secondary)] border-b border-[var(--bg-secondary)]">
                <th className="px-4 py-3 w-[160px]">Time</th>
                <th className="px-4 py-3 w-[120px]">Agent</th>
                <th className="px-4 py-3 w-[100px]">Surface</th>
                <th className="px-4 py-3 w-[130px]">Type</th>
                <th className="px-4 py-3">Summary</th>
                <th className="px-4 py-3 w-[120px]">Trace</th>
              </tr>
            </thead>
            <tbody>
              {events.map((event, idx) => {
                const url = traceUrl(event);
                const isExpanded = expandedIdx === idx;
                return (
                  <tr
                    key={`${event.event_type}-${event.agent_id}-${event.timestamp}-${idx}`}
                    className="border-b border-[var(--bg-secondary)] hover:bg-[var(--glow-secondary)] transition-colors cursor-pointer"
                    onClick={() => setExpandedIdx(isExpanded ? null : idx)}
                  >
                    <td className="px-4 py-2.5 text-[var(--text-muted)] whitespace-nowrap">
                      <TimeAbsolute timestamp={event.timestamp} />
                    </td>
                    <td className="px-4 py-2.5">
                      <AgentLink agentId={event.agent_id} />
                    </td>
                    <td className="px-4 py-2.5">
                      <span className={`inline-block px-2 py-0.5 rounded-full text-xs font-medium ${surfaceColor(event.trace_ref.surface)}`}>
                        {surfaceLabel(event.trace_ref.surface)}
                      </span>
                    </td>
                    <td className="px-4 py-2.5 text-[var(--text-muted)] font-mono text-xs">
                      {event.event_type}
                    </td>
                    <td className="px-4 py-2.5 text-[var(--text-muted)] max-w-[400px] truncate" title={event.summary}>
                      {event.summary}
                    </td>
                    <td className="px-4 py-2.5">
                      {url && (
                        <button
                          onClick={(e) => {
                            e.stopPropagation();
                            navigate(url);
                          }}
                          className="px-2 py-1 text-xs bg-[var(--glow-primary)] hover:bg-[var(--glow-primary)] text-[var(--accent-primary)] rounded-lg transition-colors whitespace-nowrap"
                          title={traceLabel(event.trace_ref.surface)}
                        >
                          Open Trace
                        </button>
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}

      <div className="text-xs text-[var(--text-secondary)] text-center">
        {events.length > 0 && `${events.length} event${events.length !== 1 ? 's' : ''}`}
        {partial && ' (partial results)'}
      </div>
    </div>
  );
}
