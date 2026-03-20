// IPC types for Phase 3.5 operator UI

export interface IpcAgent {
  agent_id: string;
  role: string | null;
  trust_level: number | null;
  status: string;
  last_seen: number | null;
  public_key: string | null;
}

export interface IpcMessage {
  id: number;
  from_agent: string;
  to_agent: string;
  kind: string;
  payload: string;
  from_trust_level: number;
  session_id: string | null;
  priority: number;
  read: boolean;
  promoted: boolean;
  blocked: boolean;
  blocked_reason: string | null;
  seq: number;
  created_at: number;
  lane: 'normal' | 'quarantine' | 'blocked';
}

export interface IpcSpawnRun {
  id: string;
  parent_id: string;
  child_id: string;
  status: string;
  result: string | null;
  created_at: number;
  expires_at: number;
  completed_at: number | null;
}

export interface IpcAuditEvent {
  timestamp: string;
  event_id: string;
  event_type: string;
  actor: {
    channel: string;
    user_id: string | null;
    username: string | null;
  } | null;
  action: {
    command: string | null;
    risk_level: string | null;
    approved: boolean;
    allowed: boolean;
  } | null;
  hmac: string | null;
}

export interface IpcAgentDetail {
  agent: IpcAgent;
  recent_messages: IpcMessage[];
  active_spawns: IpcSpawnRun[];
  quarantine_count: number;
}

export interface MessagesFilter {
  agent_id?: string;
  session_id?: string;
  kind?: string;
  quarantine?: boolean;
  dismissed?: boolean;
  lane?: string;
  from_ts?: number;
  to_ts?: number;
  limit?: number;
  offset?: number;
}

export interface SpawnRunsFilter {
  status?: string;
  parent_id?: string;
  session_id?: string;
  from_ts?: number;
  to_ts?: number;
  limit?: number;
  offset?: number;
}

export interface AuditFilter {
  agent_id?: string;
  event_type?: string;
  from_ts?: number;
  to_ts?: number;
  search?: string;
  limit?: number;
  offset?: number;
}

// ── Activity feed (Phase 3.9) ──────────────────────────────────

export interface TraceRef {
  surface: 'ipc' | 'spawn' | 'web_chat' | 'channel' | 'cron';
  session_id?: string;
  message_id?: number;
  from_agent?: string;
  to_agent?: string;
  spawn_run_id?: string;
  parent_agent_id?: string;
  child_agent_id?: string;
  chat_session_key?: string;
  run_id?: string;
  channel_name?: string;
  channel_session_key?: string;
  job_id?: string;
  job_name?: string;
}

export interface ActivityEvent {
  event_type: string;
  agent_id: string;
  timestamp: number;
  summary: string;
  trace_ref: TraceRef;
}

export interface ActivityFilter {
  agent_id?: string;
  event_type?: string;
  surface?: string;
  from_ts?: number;
  to_ts?: number;
  limit?: number;
}

export interface CronJob {
  id: string;
  name: string | null;
  command: string;
  next_run: string;
  last_run: string | null;
  last_status: string | null;
  enabled: boolean;
}

export interface CronRun {
  id: number;
  job_id: string;
  started_at: string;
  finished_at: string;
  status: string;
  output: string | null;
  duration_ms: number | null;
}
