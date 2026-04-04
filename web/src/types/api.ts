export interface StatusResponse {
  provider: string | null;
  model: string;
  summary_model: string | null;
  embedding_provider: string | null;
  embedding_model: string | null;
  temperature: number;
  uptime_seconds: number;
  gateway_port: number;
  locale: string;
  memory_backend: string;
  paired: boolean;
  channels: Record<string, boolean>;
  health: HealthSnapshot;
}

export interface HealthSnapshot {
  pid: number;
  updated_at: string;
  uptime_seconds: number;
  components: Record<string, ComponentHealth>;
}

export interface ComponentHealth {
  status: string;
  updated_at: string;
  last_ok: string | null;
  last_error: string | null;
  restart_count: number;
}

export interface ToolSpec {
  name: string;
  description: string;
  parameters: any;
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

export interface Integration {
  name: string;
  description: string;
  category: string;
  status: 'Available' | 'Active' | 'ComingSoon';
}

export interface DiagResult {
  severity: 'ok' | 'warn' | 'error';
  category: string;
  message: string;
}

export interface MemoryEntry {
  id: string;
  key: string;
  content: string;
  category: string;
  timestamp: string;
  session_id: string | null;
  score: number | null;
}

export interface MemoryCategoryStat {
  category: string;
  count: number;
}

export interface CoreBlockStat {
  label: string;
  chars: number;
  updated_at: string;
}

export interface MemoryStatsResponse {
  agent_id: string;
  total_entries: number;
  by_category: MemoryCategoryStat[];
  core_blocks: CoreBlockStat[];
  entities: number;
  skills: number;
  reflections: number;
}

export interface ContextBudgetResponse {
  recall_max_entries: number;
  recall_entry_max_chars: number;
  recall_total_max_chars: number;
  skills_max_count: number;
  skills_total_max_chars: number;
  entities_max_count: number;
  entities_total_max_chars: number;
  enrichment_total_max_chars: number;
  continuation_policy: string;
  min_relevance_score: number;
}

export interface PostTurnReportEvent extends SSEEvent {
  type: 'post_turn_report';
  agent_id: string;
  signal: string;
  explicit_mutation: boolean;
  consolidation_started: boolean;
  reflection_started: boolean;
  explicit_kind?: string | null;
}

export interface CostSummary {
  session_cost_usd: number;
  daily_cost_usd: number;
  monthly_cost_usd: number;
  total_tokens: number;
  request_count: number;
  by_model: Record<string, ModelStats>;
}

export interface ModelStats {
  model: string;
  cost_usd: number;
  total_tokens: number;
  request_count: number;
}

export interface CliTool {
  name: string;
  path: string;
  version: string | null;
  category: string;
}

export interface SSEEvent {
  type: string;
  timestamp?: string;
  [key: string]: any;
}

export interface WsMessage {
  type:
    | 'chunk'
    | 'error'
    | 'rpc_response'
    | 'session.updated'
    | 'session.deleted'
    | 'session.run_started'
    | 'session.run_finished'
    | 'session.run_interrupted'
    | 'tool_call'
    | 'tool_result'
    | 'assistant'
    | string;
  content?: string;
  full_response?: string;
  name?: string;
  args?: any;
  output?: string;
  message?: string;
  // RPC response fields
  id?: string;
  result?: any;
  error?: string;
  // Server-push event fields
  session_key?: string;
  run_id?: string;
  tool_name?: string;
}

export interface ChatSessionInfo {
  key: string;
  kind?: 'web' | 'channel' | 'ipc';
  channel?: string | null;
  label: string | null;
  last_active: number;
  message_count: number;
  preview: string | null;
  has_active_run: boolean;
  input_tokens: number;
  output_tokens: number;
  current_goal: string | null;
  session_summary: string | null;
}

export interface ChatMessageInfo {
  id: number;
  event_type: string;
  role: string | null;
  content: string;
  tool_name: string | null;
  run_id: string | null;
  timestamp: number;
  input_tokens: number | null;
  output_tokens: number | null;
}

export interface ChannelSessionInfo {
  key: string;
  channel: string;
  sender: string;
  created_at: number;
  last_activity: number;
  message_count: number;
  summary: string | null;
}

export interface ChannelMessageInfo {
  role: string;
  content: string;
}
