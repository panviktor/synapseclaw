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

DEFINE INDEX IF NOT EXISTS idx_ep_agent   ON episode FIELDS agent_id;
DEFINE INDEX IF NOT EXISTS idx_ep_session ON episode FIELDS session_id;
DEFINE INDEX IF NOT EXISTS idx_ep_time    ON episode FIELDS created_at;
DEFINE ANALYZER IF NOT EXISTS simple_analyzer TOKENIZERS blank, class FILTERS lowercase;
DEFINE INDEX IF NOT EXISTS idx_ep_content ON episode FIELDS content
    SEARCH ANALYZER simple_analyzer BM25;

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

DEFINE INDEX IF NOT EXISTS idx_ent_name ON entity FIELDS name;
DEFINE INDEX IF NOT EXISTS idx_ent_type ON entity FIELDS entity_type;

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

DEFINE INDEX IF NOT EXISTS idx_skill_name ON skill FIELDS name;

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

DEFINE INDEX IF NOT EXISTS idx_refl_agent ON reflection FIELDS agent_id;
