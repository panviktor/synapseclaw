-- SynapseClaw Phase 4.3: Memory Architecture Schema
-- Applied on every startup (idempotent via DEFINE ... IF NOT EXISTS)

-- ═══════════════════════════════════════════
-- EPISODIC MEMORY (raw interactions)
-- ═══════════════════════════════════════════

DEFINE TABLE IF NOT EXISTS episode SCHEMALESS;
DEFINE FIELD IF NOT EXISTS agent_id    ON episode TYPE option<string>;
DEFINE FIELD IF NOT EXISTS session_id  ON episode TYPE option<string>;
DEFINE FIELD IF NOT EXISTS key         ON episode TYPE option<string>;
DEFINE FIELD IF NOT EXISTS content     ON episode TYPE option<string>;
DEFINE FIELD IF NOT EXISTS category    ON episode TYPE option<string>;
DEFINE FIELD IF NOT EXISTS importance  ON episode TYPE option<float>;
DEFINE FIELD IF NOT EXISTS created_at  ON episode TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS visibility  ON episode TYPE option<string>;
DEFINE FIELD IF NOT EXISTS embedding   ON episode TYPE option<array<float>>;
DEFINE FIELD IF NOT EXISTS embedding_profile_id ON episode TYPE option<string>;
DEFINE FIELD IF NOT EXISTS access_count ON episode TYPE option<int> DEFAULT 0;
DEFINE FIELD IF NOT EXISTS last_accessed ON episode TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS shared_with ON episode TYPE option<array<string>>;

DEFINE INDEX IF NOT EXISTS idx_ep_agent   ON episode FIELDS agent_id;
DEFINE INDEX IF NOT EXISTS idx_ep_session ON episode FIELDS session_id;
DEFINE INDEX IF NOT EXISTS idx_ep_time    ON episode FIELDS created_at;
DEFINE INDEX IF NOT EXISTS idx_ep_key     ON episode FIELDS key;
DEFINE ANALYZER IF NOT EXISTS simple_analyzer TOKENIZERS blank, class FILTERS lowercase;

-- ═══════════════════════════════════════════
-- CORE MEMORY BLOCKS (MemGPT pattern)
-- ═══════════════════════════════════════════

DEFINE TABLE IF NOT EXISTS core_memory SCHEMALESS;
DEFINE FIELD IF NOT EXISTS agent_id   ON core_memory TYPE option<string>;
DEFINE FIELD IF NOT EXISTS label      ON core_memory TYPE option<string>;
DEFINE FIELD IF NOT EXISTS content    ON core_memory TYPE option<string>;
DEFINE FIELD IF NOT EXISTS max_tokens ON core_memory TYPE option<int>;
DEFINE FIELD IF NOT EXISTS updated_at ON core_memory TYPE option<datetime>;

DEFINE INDEX IF NOT EXISTS idx_cm_agent ON core_memory FIELDS agent_id, label UNIQUE;

-- ═══════════════════════════════════════════
-- ENTITIES (Knowledge Graph nodes)
-- ═══════════════════════════════════════════

DEFINE TABLE IF NOT EXISTS entity SCHEMALESS;
DEFINE FIELD IF NOT EXISTS name        ON entity TYPE option<string>;
DEFINE FIELD IF NOT EXISTS entity_type ON entity TYPE option<string>;
DEFINE FIELD IF NOT EXISTS properties  ON entity TYPE option<object>;
DEFINE FIELD IF NOT EXISTS summary     ON entity TYPE option<string>;
DEFINE FIELD IF NOT EXISTS created_by  ON entity TYPE option<string>;
DEFINE FIELD IF NOT EXISTS created_at  ON entity TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS updated_at  ON entity TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS embedding   ON entity TYPE option<array<float>>;
DEFINE FIELD IF NOT EXISTS embedding_profile_id ON entity TYPE option<string>;
DEFINE FIELD IF NOT EXISTS visibility  ON entity TYPE option<string>;
DEFINE FIELD IF NOT EXISTS shared_with ON entity TYPE option<array<string>>;

DEFINE INDEX IF NOT EXISTS idx_ent_name ON entity FIELDS name;
DEFINE INDEX IF NOT EXISTS idx_ent_type ON entity FIELDS entity_type;
-- HNSW vector index created separately via apply_vector_indexes()

-- ═══════════════════════════════════════════
-- FACTS (Knowledge Graph edges, bitemporal)
-- ═══════════════════════════════════════════

DEFINE TABLE IF NOT EXISTS fact SCHEMALESS;
DEFINE FIELD IF NOT EXISTS subject        ON fact TYPE option<string>;
DEFINE FIELD IF NOT EXISTS predicate      ON fact TYPE option<string>;
DEFINE FIELD IF NOT EXISTS object         ON fact TYPE option<string>;
DEFINE FIELD IF NOT EXISTS confidence     ON fact TYPE option<float>;
DEFINE FIELD IF NOT EXISTS valid_from     ON fact TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS valid_to       ON fact TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS recorded_at    ON fact TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS source_episode ON fact TYPE option<string>;
DEFINE FIELD IF NOT EXISTS created_by     ON fact TYPE option<string>;
DEFINE FIELD IF NOT EXISTS embedding      ON fact TYPE option<array<float>>;
DEFINE FIELD IF NOT EXISTS embedding_profile_id ON fact TYPE option<string>;
DEFINE FIELD IF NOT EXISTS visibility     ON fact TYPE option<string>;
DEFINE FIELD IF NOT EXISTS shared_with    ON fact TYPE option<array<string>>;

DEFINE INDEX IF NOT EXISTS idx_fact_subj ON fact FIELDS subject;
DEFINE INDEX IF NOT EXISTS idx_fact_obj  ON fact FIELDS object;
DEFINE INDEX IF NOT EXISTS idx_fact_pred ON fact FIELDS predicate;

-- ═══════════════════════════════════════════
-- SKILLS (procedural memory)
-- ═══════════════════════════════════════════

DEFINE TABLE IF NOT EXISTS skill SCHEMALESS;
DEFINE FIELD IF NOT EXISTS name          ON skill TYPE option<string>;
DEFINE FIELD IF NOT EXISTS description   ON skill TYPE option<string>;
DEFINE FIELD IF NOT EXISTS content       ON skill TYPE option<string>;
DEFINE FIELD IF NOT EXISTS task_family   ON skill TYPE option<string>;
DEFINE FIELD IF NOT EXISTS tool_pattern  ON skill TYPE option<array>;
DEFINE FIELD IF NOT EXISTS lineage_task_families ON skill TYPE option<array>;
DEFINE FIELD IF NOT EXISTS tags          ON skill TYPE option<array>;
DEFINE FIELD IF NOT EXISTS success_count ON skill TYPE option<int>;
DEFINE FIELD IF NOT EXISTS fail_count    ON skill TYPE option<int>;
DEFINE FIELD IF NOT EXISTS version       ON skill TYPE option<int>;
DEFINE FIELD IF NOT EXISTS created_by    ON skill TYPE option<string>;
DEFINE FIELD IF NOT EXISTS created_at    ON skill TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS updated_at    ON skill TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS embedding     ON skill TYPE option<array<float>>;
DEFINE FIELD IF NOT EXISTS visibility    ON skill TYPE option<string>;
DEFINE FIELD IF NOT EXISTS shared_with   ON skill TYPE option<array<string>>;

DEFINE INDEX IF NOT EXISTS idx_skill_name ON skill FIELDS name;
-- HNSW vector index created separately via apply_vector_indexes()

-- ═══════════════════════════════════════════
-- REFLECTIONS (self-improvement records)
-- ═══════════════════════════════════════════

DEFINE TABLE IF NOT EXISTS reflection SCHEMALESS;
DEFINE FIELD IF NOT EXISTS agent_id      ON reflection TYPE option<string>;
DEFINE FIELD IF NOT EXISTS pipeline_run  ON reflection TYPE option<string>;
DEFINE FIELD IF NOT EXISTS task_summary  ON reflection TYPE option<string>;
DEFINE FIELD IF NOT EXISTS outcome       ON reflection TYPE option<string>;
DEFINE FIELD IF NOT EXISTS what_worked   ON reflection TYPE option<string>;
DEFINE FIELD IF NOT EXISTS what_failed   ON reflection TYPE option<string>;
DEFINE FIELD IF NOT EXISTS lesson        ON reflection TYPE option<string>;
DEFINE FIELD IF NOT EXISTS created_at    ON reflection TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS embedding     ON reflection TYPE option<array<float>>;
DEFINE FIELD IF NOT EXISTS visibility    ON reflection TYPE option<string>;
DEFINE FIELD IF NOT EXISTS shared_with   ON reflection TYPE option<array<string>>;

DEFINE INDEX IF NOT EXISTS idx_refl_agent ON reflection FIELDS agent_id;
-- HNSW vector index created separately via apply_vector_indexes()

-- ═══════════════════════════════════════════
-- DEAD LETTER QUEUE (Phase 4.5: failed pipeline steps)
-- ═══════════════════════════════════════════

DEFINE TABLE IF NOT EXISTS dead_letter SCHEMALESS;
DEFINE FIELD IF NOT EXISTS dl_id           ON dead_letter TYPE option<string>;
DEFINE FIELD IF NOT EXISTS pipeline_run_id ON dead_letter TYPE option<string>;
DEFINE FIELD IF NOT EXISTS step_id         ON dead_letter TYPE option<string>;
DEFINE FIELD IF NOT EXISTS agent_id        ON dead_letter TYPE option<string>;
DEFINE FIELD IF NOT EXISTS input           ON dead_letter TYPE option<object>;
DEFINE FIELD IF NOT EXISTS error           ON dead_letter TYPE option<string>;
DEFINE FIELD IF NOT EXISTS attempt         ON dead_letter TYPE option<int>;
DEFINE FIELD IF NOT EXISTS max_retries     ON dead_letter TYPE option<int>;
DEFINE FIELD IF NOT EXISTS created_at      ON dead_letter TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS status          ON dead_letter TYPE option<string>;
DEFINE FIELD IF NOT EXISTS retried_at      ON dead_letter TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS dismissed_by    ON dead_letter TYPE option<string>;

DEFINE INDEX IF NOT EXISTS idx_dl_status  ON dead_letter FIELDS status;
DEFINE INDEX IF NOT EXISTS idx_dl_created ON dead_letter FIELDS created_at;
DEFINE INDEX IF NOT EXISTS idx_dl_run     ON dead_letter FIELDS pipeline_run_id;

-- ═══════════════════════════════════════════
-- CHAT SESSIONS (Phase 4.5: migrated from SQLite ChatDb)
-- ═══════════════════════════════════════════

DEFINE TABLE IF NOT EXISTS chat_session SCHEMALESS;
DEFINE FIELD IF NOT EXISTS key             ON chat_session TYPE option<string>;
DEFINE FIELD IF NOT EXISTS label           ON chat_session TYPE option<string>;
DEFINE FIELD IF NOT EXISTS current_goal    ON chat_session TYPE option<string>;
DEFINE FIELD IF NOT EXISTS session_summary ON chat_session TYPE option<string>;
DEFINE FIELD IF NOT EXISTS created_at      ON chat_session TYPE option<int>;
DEFINE FIELD IF NOT EXISTS last_active     ON chat_session TYPE option<int>;
DEFINE FIELD IF NOT EXISTS message_count   ON chat_session TYPE option<int>;
DEFINE FIELD IF NOT EXISTS input_tokens    ON chat_session TYPE option<int>;
DEFINE FIELD IF NOT EXISTS output_tokens   ON chat_session TYPE option<int>;

DEFINE INDEX IF NOT EXISTS idx_cs_key     ON chat_session FIELDS key UNIQUE;
DEFINE INDEX IF NOT EXISTS idx_cs_active  ON chat_session FIELDS last_active;

-- ═══════════════════════════════════════════
-- CHAT MESSAGES (Phase 4.5: migrated from SQLite ChatDb)
-- ═══════════════════════════════════════════

DEFINE TABLE IF NOT EXISTS chat_message SCHEMALESS;
DEFINE FIELD IF NOT EXISTS session_key    ON chat_message TYPE option<string>;
DEFINE FIELD IF NOT EXISTS kind           ON chat_message TYPE option<string>;
DEFINE FIELD IF NOT EXISTS role           ON chat_message TYPE option<string>;
DEFINE FIELD IF NOT EXISTS content        ON chat_message TYPE option<string>;
DEFINE FIELD IF NOT EXISTS tool_name      ON chat_message TYPE option<string>;
DEFINE FIELD IF NOT EXISTS run_id         ON chat_message TYPE option<string>;
DEFINE FIELD IF NOT EXISTS input_tokens   ON chat_message TYPE option<int>;
DEFINE FIELD IF NOT EXISTS output_tokens  ON chat_message TYPE option<int>;
DEFINE FIELD IF NOT EXISTS timestamp      ON chat_message TYPE option<int>;
DEFINE FIELD IF NOT EXISTS seq            ON chat_message TYPE option<int>;

DEFINE INDEX IF NOT EXISTS idx_cm_session ON chat_message FIELDS session_key, timestamp;

-- ═══════════════════════════════════════════
-- RUNS (Phase 4.5: migrated from SQLite ChatDb)
-- ═══════════════════════════════════════════

DEFINE TABLE IF NOT EXISTS run SCHEMALESS;
DEFINE FIELD IF NOT EXISTS run_id            ON run TYPE option<string>;
DEFINE FIELD IF NOT EXISTS conversation_key  ON run TYPE option<string>;
DEFINE FIELD IF NOT EXISTS origin            ON run TYPE option<string>;
DEFINE FIELD IF NOT EXISTS state             ON run TYPE option<string>;
DEFINE FIELD IF NOT EXISTS started_at        ON run TYPE option<int>;
DEFINE FIELD IF NOT EXISTS finished_at       ON run TYPE option<int>;
DEFINE FIELD IF NOT EXISTS created_at        ON run TYPE option<int>;

DEFINE INDEX IF NOT EXISTS idx_run_id   ON run FIELDS run_id UNIQUE;
DEFINE INDEX IF NOT EXISTS idx_run_conv ON run FIELDS conversation_key;

-- ═══════════════════════════════════════════
-- RUN EVENTS (Phase 4.5: migrated from SQLite ChatDb)
-- ═══════════════════════════════════════════

DEFINE TABLE IF NOT EXISTS run_event SCHEMALESS;
DEFINE FIELD IF NOT EXISTS run_id      ON run_event TYPE option<string>;
DEFINE FIELD IF NOT EXISTS event_type  ON run_event TYPE option<string>;
DEFINE FIELD IF NOT EXISTS content     ON run_event TYPE option<string>;
DEFINE FIELD IF NOT EXISTS tool_name   ON run_event TYPE option<string>;
DEFINE FIELD IF NOT EXISTS created_at  ON run_event TYPE option<int>;

DEFINE INDEX IF NOT EXISTS idx_re_run ON run_event FIELDS run_id, created_at;

-- ═══════════════════════════════════════════
-- HEARTBEAT RUNS (Phase 4.5: migrated from SQLite)
-- ═══════════════════════════════════════════

DEFINE TABLE IF NOT EXISTS heartbeat_run SCHEMALESS;
DEFINE FIELD IF NOT EXISTS task_text     ON heartbeat_run TYPE option<string>;
DEFINE FIELD IF NOT EXISTS task_priority ON heartbeat_run TYPE option<string>;
DEFINE FIELD IF NOT EXISTS started_at    ON heartbeat_run TYPE option<string>;
DEFINE FIELD IF NOT EXISTS finished_at   ON heartbeat_run TYPE option<string>;
DEFINE FIELD IF NOT EXISTS status        ON heartbeat_run TYPE option<string>;
DEFINE FIELD IF NOT EXISTS output        ON heartbeat_run TYPE option<string>;
DEFINE FIELD IF NOT EXISTS duration_ms   ON heartbeat_run TYPE option<int>;
DEFINE INDEX IF NOT EXISTS idx_hb_started ON heartbeat_run FIELDS started_at;

-- ═══════════════════════════════════════════
-- CHANNEL SESSIONS (migrated from SQLite)
-- ═══════════════════════════════════════════

DEFINE TABLE IF NOT EXISTS channel_session SCHEMALESS;
DEFINE FIELD IF NOT EXISTS session_key ON channel_session TYPE option<string>;
DEFINE FIELD IF NOT EXISTS role ON channel_session TYPE option<string>;
DEFINE FIELD IF NOT EXISTS content ON channel_session TYPE option<string>;
DEFINE FIELD IF NOT EXISTS created_at ON channel_session TYPE option<string>;
DEFINE INDEX IF NOT EXISTS idx_chs_key ON channel_session FIELDS session_key;

-- ═══════════════════════════════════════════
-- CHANNEL SESSION METADATA
-- ═══════════════════════════════════════════

DEFINE TABLE IF NOT EXISTS channel_session_meta SCHEMALESS;
DEFINE FIELD IF NOT EXISTS session_key ON channel_session_meta TYPE option<string>;
DEFINE FIELD IF NOT EXISTS label ON channel_session_meta TYPE option<string>;
DEFINE FIELD IF NOT EXISTS current_goal ON channel_session_meta TYPE option<string>;
DEFINE FIELD IF NOT EXISTS created_at ON channel_session_meta TYPE option<string>;
DEFINE FIELD IF NOT EXISTS last_activity ON channel_session_meta TYPE option<string>;
DEFINE FIELD IF NOT EXISTS message_count ON channel_session_meta TYPE option<int>;
DEFINE FIELD IF NOT EXISTS input_tokens ON channel_session_meta TYPE option<int>;
DEFINE FIELD IF NOT EXISTS output_tokens ON channel_session_meta TYPE option<int>;
DEFINE INDEX IF NOT EXISTS idx_csm_key ON channel_session_meta FIELDS session_key UNIQUE;

-- ═══════════════════════════════════════════
-- CHANNEL SESSION SUMMARIES
-- ═══════════════════════════════════════════

DEFINE TABLE IF NOT EXISTS channel_session_summary SCHEMALESS;
DEFINE FIELD IF NOT EXISTS session_key ON channel_session_summary TYPE option<string>;
DEFINE FIELD IF NOT EXISTS summary ON channel_session_summary TYPE option<string>;
DEFINE FIELD IF NOT EXISTS message_count_at_summary ON channel_session_summary TYPE option<int>;
DEFINE FIELD IF NOT EXISTS updated_at ON channel_session_summary TYPE option<string>;
DEFINE INDEX IF NOT EXISTS idx_css_key ON channel_session_summary FIELDS session_key UNIQUE;

-- ═══════════════════════════════════════════
-- CRON JOBS (migrated from SQLite)
-- ═══════════════════════════════════════════

DEFINE TABLE IF NOT EXISTS cron_job SCHEMALESS;
DEFINE FIELD IF NOT EXISTS job_id ON cron_job TYPE option<string>;
DEFINE FIELD IF NOT EXISTS expression ON cron_job TYPE option<string>;
DEFINE FIELD IF NOT EXISTS command ON cron_job TYPE option<string>;
DEFINE FIELD IF NOT EXISTS schedule ON cron_job TYPE option<string>;
DEFINE FIELD IF NOT EXISTS job_type ON cron_job TYPE option<string>;
DEFINE FIELD IF NOT EXISTS prompt ON cron_job TYPE option<string>;
DEFINE FIELD IF NOT EXISTS name ON cron_job TYPE option<string>;
DEFINE FIELD IF NOT EXISTS session_target ON cron_job TYPE option<string>;
DEFINE FIELD IF NOT EXISTS model ON cron_job TYPE option<string>;
DEFINE FIELD IF NOT EXISTS enabled ON cron_job TYPE option<bool>;
DEFINE FIELD IF NOT EXISTS delivery ON cron_job TYPE option<string>;
DEFINE FIELD IF NOT EXISTS delete_after_run ON cron_job TYPE option<bool>;
DEFINE FIELD IF NOT EXISTS execution_mode ON cron_job TYPE option<string>;
DEFINE FIELD IF NOT EXISTS env_overlay ON cron_job TYPE option<string>;
DEFINE FIELD IF NOT EXISTS created_at ON cron_job TYPE option<string>;
DEFINE FIELD IF NOT EXISTS next_run ON cron_job TYPE option<string>;
DEFINE FIELD IF NOT EXISTS last_run ON cron_job TYPE option<string>;
DEFINE FIELD IF NOT EXISTS last_status ON cron_job TYPE option<string>;
DEFINE FIELD IF NOT EXISTS last_output ON cron_job TYPE option<string>;
DEFINE INDEX IF NOT EXISTS idx_cj_id ON cron_job FIELDS job_id UNIQUE;
DEFINE INDEX IF NOT EXISTS idx_cj_next ON cron_job FIELDS next_run;

-- ═══════════════════════════════════════════
-- CRON RUNS (migrated from SQLite)
-- ═══════════════════════════════════════════

DEFINE TABLE IF NOT EXISTS cron_run SCHEMALESS;
DEFINE FIELD IF NOT EXISTS job_id ON cron_run TYPE option<string>;
DEFINE FIELD IF NOT EXISTS started_at ON cron_run TYPE option<string>;
DEFINE FIELD IF NOT EXISTS finished_at ON cron_run TYPE option<string>;
DEFINE FIELD IF NOT EXISTS status ON cron_run TYPE option<string>;
DEFINE FIELD IF NOT EXISTS output ON cron_run TYPE option<string>;
DEFINE FIELD IF NOT EXISTS duration_ms ON cron_run TYPE option<int>;
DEFINE INDEX IF NOT EXISTS idx_cr_job ON cron_run FIELDS job_id;
DEFINE INDEX IF NOT EXISTS idx_cr_started ON cron_run FIELDS started_at;

-- ═══════════════════════════════════════════
-- RESPONSE CACHE (migrated from SQLite)
-- ═══════════════════════════════════════════

DEFINE TABLE IF NOT EXISTS response_cache SCHEMALESS;
DEFINE FIELD IF NOT EXISTS prompt_hash ON response_cache TYPE option<string>;
DEFINE FIELD IF NOT EXISTS model       ON response_cache TYPE option<string>;
DEFINE FIELD IF NOT EXISTS response    ON response_cache TYPE option<string>;
DEFINE FIELD IF NOT EXISTS token_count ON response_cache TYPE option<int>;
DEFINE FIELD IF NOT EXISTS created_at  ON response_cache TYPE option<string>;
DEFINE FIELD IF NOT EXISTS accessed_at ON response_cache TYPE option<string>;
DEFINE FIELD IF NOT EXISTS hit_count   ON response_cache TYPE option<int>;
DEFINE INDEX IF NOT EXISTS idx_rc_hash     ON response_cache FIELDS prompt_hash UNIQUE;
DEFINE INDEX IF NOT EXISTS idx_rc_accessed ON response_cache FIELDS accessed_at;

-- ═══════════════════════════════════════════
-- IPC AGENTS (Phase 4.5: migrated from SQLite IpcDb)
-- ═══════════════════════════════════════════

DEFINE TABLE IF NOT EXISTS ipc_agent SCHEMALESS;
DEFINE FIELD IF NOT EXISTS agent_id      ON ipc_agent TYPE option<string>;
DEFINE FIELD IF NOT EXISTS role          ON ipc_agent TYPE option<string>;
DEFINE FIELD IF NOT EXISTS trust_level   ON ipc_agent TYPE option<int>;
DEFINE FIELD IF NOT EXISTS status        ON ipc_agent TYPE option<string>;
DEFINE FIELD IF NOT EXISTS metadata      ON ipc_agent TYPE option<string>;
DEFINE FIELD IF NOT EXISTS last_seen     ON ipc_agent TYPE option<int>;
DEFINE FIELD IF NOT EXISTS public_key    ON ipc_agent TYPE option<string>;
DEFINE INDEX IF NOT EXISTS idx_ia_id     ON ipc_agent FIELDS agent_id UNIQUE;

-- IPC MESSAGES
DEFINE TABLE IF NOT EXISTS ipc_message SCHEMALESS;
DEFINE FIELD IF NOT EXISTS session_id          ON ipc_message TYPE option<string>;
DEFINE FIELD IF NOT EXISTS from_agent          ON ipc_message TYPE option<string>;
DEFINE FIELD IF NOT EXISTS to_agent            ON ipc_message TYPE option<string>;
DEFINE FIELD IF NOT EXISTS kind                ON ipc_message TYPE option<string>;
DEFINE FIELD IF NOT EXISTS payload             ON ipc_message TYPE option<string>;
DEFINE FIELD IF NOT EXISTS priority            ON ipc_message TYPE option<int>;
DEFINE FIELD IF NOT EXISTS from_trust_level    ON ipc_message TYPE option<int>;
DEFINE FIELD IF NOT EXISTS seq                 ON ipc_message TYPE option<int>;
DEFINE FIELD IF NOT EXISTS blocked             ON ipc_message TYPE option<int>;
DEFINE FIELD IF NOT EXISTS block_reason        ON ipc_message TYPE option<string>;
DEFINE FIELD IF NOT EXISTS created_at          ON ipc_message TYPE option<int>;
DEFINE FIELD IF NOT EXISTS read                ON ipc_message TYPE option<int>;
DEFINE FIELD IF NOT EXISTS expires_at          ON ipc_message TYPE option<int>;
DEFINE FIELD IF NOT EXISTS delivery_status     ON ipc_message TYPE option<string>;
DEFINE FIELD IF NOT EXISTS promoted            ON ipc_message TYPE option<int>;
DEFINE FIELD IF NOT EXISTS msg_id              ON ipc_message TYPE option<int>;
DEFINE INDEX IF NOT EXISTS idx_im_inbox        ON ipc_message FIELDS to_agent, read, created_at;
DEFINE INDEX IF NOT EXISTS idx_im_session      ON ipc_message FIELDS session_id;

-- IPC SHARED STATE
DEFINE TABLE IF NOT EXISTS ipc_shared_state SCHEMALESS;
DEFINE FIELD IF NOT EXISTS key         ON ipc_shared_state TYPE option<string>;
DEFINE FIELD IF NOT EXISTS value       ON ipc_shared_state TYPE option<string>;
DEFINE FIELD IF NOT EXISTS owner       ON ipc_shared_state TYPE option<string>;
DEFINE FIELD IF NOT EXISTS updated_at  ON ipc_shared_state TYPE option<int>;
DEFINE INDEX IF NOT EXISTS idx_iss_key ON ipc_shared_state FIELDS key UNIQUE;

-- IPC SEQUENCES
DEFINE TABLE IF NOT EXISTS ipc_message_seq SCHEMALESS;
DEFINE FIELD IF NOT EXISTS agent_id    ON ipc_message_seq TYPE option<string>;
DEFINE FIELD IF NOT EXISTS last_seq    ON ipc_message_seq TYPE option<int>;
DEFINE INDEX IF NOT EXISTS idx_ims_id  ON ipc_message_seq FIELDS agent_id UNIQUE;

DEFINE TABLE IF NOT EXISTS ipc_sender_seq SCHEMALESS;
DEFINE FIELD IF NOT EXISTS agent_id          ON ipc_sender_seq TYPE option<string>;
DEFINE FIELD IF NOT EXISTS last_sender_seq   ON ipc_sender_seq TYPE option<int>;
DEFINE INDEX IF NOT EXISTS idx_iss2_id       ON ipc_sender_seq FIELDS agent_id UNIQUE;

-- IPC SPAWN RUNS
DEFINE TABLE IF NOT EXISTS ipc_spawn_run SCHEMALESS;
DEFINE FIELD IF NOT EXISTS run_id       ON ipc_spawn_run TYPE option<string>;
DEFINE FIELD IF NOT EXISTS parent_id    ON ipc_spawn_run TYPE option<string>;
DEFINE FIELD IF NOT EXISTS child_id     ON ipc_spawn_run TYPE option<string>;
DEFINE FIELD IF NOT EXISTS status       ON ipc_spawn_run TYPE option<string>;
DEFINE FIELD IF NOT EXISTS result       ON ipc_spawn_run TYPE option<string>;
DEFINE FIELD IF NOT EXISTS created_at   ON ipc_spawn_run TYPE option<int>;
DEFINE FIELD IF NOT EXISTS expires_at   ON ipc_spawn_run TYPE option<int>;
DEFINE FIELD IF NOT EXISTS completed_at ON ipc_spawn_run TYPE option<int>;
DEFINE INDEX IF NOT EXISTS idx_isr_id     ON ipc_spawn_run FIELDS run_id UNIQUE;
DEFINE INDEX IF NOT EXISTS idx_isr_parent ON ipc_spawn_run FIELDS parent_id, status;
DEFINE INDEX IF NOT EXISTS idx_isr_child  ON ipc_spawn_run FIELDS child_id;

-- IPC AGENT GATEWAYS
DEFINE TABLE IF NOT EXISTS ipc_agent_gateway SCHEMALESS;
DEFINE FIELD IF NOT EXISTS agent_id       ON ipc_agent_gateway TYPE option<string>;
DEFINE FIELD IF NOT EXISTS gateway_url    ON ipc_agent_gateway TYPE option<string>;
DEFINE FIELD IF NOT EXISTS proxy_token    ON ipc_agent_gateway TYPE option<string>;
DEFINE FIELD IF NOT EXISTS registered_at  ON ipc_agent_gateway TYPE option<int>;
DEFINE INDEX IF NOT EXISTS idx_iag_id     ON ipc_agent_gateway FIELDS agent_id UNIQUE;

-- ═══════════════════════════════════════════
-- LEARNING SIGNAL PATTERNS (configurable via UI)
-- ═══════════════════════════════════════════

DEFINE TABLE IF NOT EXISTS learning_signal_pattern SCHEMALESS;
DEFINE FIELD IF NOT EXISTS signal_type ON learning_signal_pattern TYPE option<string>;
DEFINE FIELD IF NOT EXISTS pattern     ON learning_signal_pattern TYPE option<string>;
DEFINE FIELD IF NOT EXISTS match_mode  ON learning_signal_pattern TYPE option<string>;
DEFINE FIELD IF NOT EXISTS language    ON learning_signal_pattern TYPE option<string>;
DEFINE FIELD IF NOT EXISTS enabled     ON learning_signal_pattern TYPE option<bool> DEFAULT true;
DEFINE FIELD IF NOT EXISTS created_at  ON learning_signal_pattern TYPE option<datetime>;
DEFINE INDEX IF NOT EXISTS idx_lsp_type ON learning_signal_pattern FIELDS signal_type;

-- ═══════════════════════════════════════════
-- USER PROFILE FACTS (dynamic, schemaless)
-- ═══════════════════════════════════════════

DEFINE TABLE IF NOT EXISTS user_profile SCHEMALESS;
DEFINE FIELD IF NOT EXISTS user_key   ON user_profile TYPE option<string>;
DEFINE FIELD IF NOT EXISTS facts      ON user_profile TYPE option<object>;
DEFINE FIELD IF NOT EXISTS created_at ON user_profile TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS updated_at ON user_profile TYPE option<datetime>;
DEFINE INDEX IF NOT EXISTS idx_user_profile_key ON user_profile FIELDS user_key UNIQUE;
