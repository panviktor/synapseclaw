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
  nearby_max_entries: number;
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

export interface ProjectionRef {
  projection: string | null;
  key?: string;
  kind?: string;
  task_family?: string;
  representative_key?: string;
  representative_task_family?: string;
  member_count?: number;
  member_keys?: string[];
  member_task_families?: string[];
  lineage_task_families?: string[];
}

export interface SkillSurfaceEntry {
  name: string;
  status: string;
  source: string;
  priority: number;
  origin: string;
  effective: boolean;
  shadowed_by: string | null;
  projection: string | null;
}

export interface UserProfileProjectionResponse {
  key: string;
  projection: string;
}

export interface WorkingStateProjectionResponse {
  session_key: string;
  projection: string;
}

export interface ProceduralContradictionResponse {
  recipe_task_family: string;
  recipe_lineage_task_families: string[];
  recipe_cluster_size: number;
  failure_representative_key: string;
  failure_cluster_size: number;
  failed_tools: string[];
  overlap: number;
}

export interface ProceduralClusterReviewResponse {
  kind: string;
  representative_key: string;
  member_count: number;
  action: string;
  reason: string;
}

export interface LearningMaintenanceSnapshotResponse {
  recent_run_recipe_count: number;
  run_recipe_cluster_count: number;
  procedural_contradiction_count: number;
  recent_precedent_count: number;
  precedent_cluster_count: number;
  precedent_compact_candidate_count: number;
  precedent_preserve_branch_count: number;
  recent_reflection_count: number;
  recent_failure_pattern_count: number;
  failure_pattern_cluster_count: number;
  failure_pattern_compact_candidate_count: number;
  failure_pattern_blocking_count: number;
  recent_skill_count: number;
  candidate_skill_count: number;
  skipped_cycles_since_maintenance: number;
  prompt_optimization_due: boolean;
}

export interface LearningMaintenancePlanResponse {
  run_importance_decay: boolean;
  run_gc: boolean;
  run_run_recipe_review: boolean;
  run_precedent_compaction: boolean;
  run_failure_pattern_compaction: boolean;
  run_skill_review: boolean;
  run_prompt_optimization: boolean;
  reasons: string[];
}

export interface SkillReviewDecisionResponse {
  skill_id: string;
  skill_name: string;
  lineage_task_families: string[];
  action: string;
  target_status: string;
  reason: string;
}

export interface RunRecipeResponse {
  agent_id: string;
  task_family: string;
  sample_request: string;
  summary: string;
  tool_pattern: string[];
  lineage_task_families: string[];
  success_count: number;
  updated_at: number;
}

export interface RunRecipeReviewDecisionResponse {
  canonical_recipe: RunRecipeResponse;
  removed_task_families: string[];
  cluster_task_families: string[];
  reason: string;
  promotion_blocked: boolean;
  promotion_block_reason: string | null;
}

export interface MemoryProjectionsResponse {
  agent_id: string;
  current_user_profile: UserProfileProjectionResponse | null;
  learning_digest: string | null;
  learning_maintenance: string | null;
  learning_maintenance_snapshot: LearningMaintenanceSnapshotResponse;
  learning_maintenance_plan: LearningMaintenancePlanResponse;
  procedural_contradictions: ProceduralContradictionResponse[];
  procedural_contradiction_projection: string | null;
  procedural_cluster_review: string | null;
  precedent_cluster_reviews: ProceduralClusterReviewResponse[];
  failure_pattern_cluster_reviews: ProceduralClusterReviewResponse[];
  core_memory: string | null;
  working_state: WorkingStateProjectionResponse | null;
  recent_sessions: ProjectionRef[];
  skill_conflict_policy: string | null;
  skill_review: string | null;
  skill_review_decisions: SkillReviewDecisionResponse[];
  run_recipe_review: string | null;
  run_recipe_review_decisions: RunRecipeReviewDecisionResponse[];
  configured_skills: SkillSurfaceEntry[];
  recent_skills: SkillSurfaceEntry[];
  skill_surface: SkillSurfaceEntry[];
  effective_skills: SkillSurfaceEntry[];
  run_recipes: ProjectionRef[];
  recipe_clusters: ProjectionRef[];
  recent_precedents: ProjectionRef[];
  precedent_clusters: ProjectionRef[];
  recent_reflections: ProjectionRef[];
  recent_failure_patterns: ProjectionRef[];
  failure_pattern_clusters: ProjectionRef[];
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
