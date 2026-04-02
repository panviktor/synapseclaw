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
DEFINE FIELD IF NOT EXISTS tags          ON skill TYPE option<array>;
DEFINE FIELD IF NOT EXISTS success_count ON skill TYPE option<int>;
DEFINE FIELD IF NOT EXISTS fail_count    ON skill TYPE option<int>;
DEFINE FIELD IF NOT EXISTS version       ON skill TYPE option<int>;
DEFINE FIELD IF NOT EXISTS created_by    ON skill TYPE option<string>;
DEFINE FIELD IF NOT EXISTS created_at    ON skill TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS updated_at    ON skill TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS embedding     ON skill TYPE option<array<float>>;

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

DEFINE INDEX IF NOT EXISTS idx_refl_agent ON reflection FIELDS agent_id;
-- HNSW vector index created separately via apply_vector_indexes()

-- ═══════════════════════════════════════════
-- DEAD LETTER QUEUE (Phase 4.5: failed pipeline steps)
-- ═══════════════════════════════════════════

DEFINE TABLE IF NOT EXISTS dead_letter SCHEMALESS;
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
