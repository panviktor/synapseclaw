# Phase 4.4: Prompt Optimizer — self-improving agent instructions

## Goal

Agents automatically improve their own instructions by analyzing accumulated reflections (what worked, what failed, lessons learned) and applying targeted changes to core memory blocks. Implements the LangMem gradient + Letta sleep-time hybrid pattern.

---

## Problem

Phase 4.3 gave agents memory — they remember facts, learn skills, build knowledge graphs. But this knowledge **doesn't change how agents behave**. The system prompt (SOUL.md + core blocks) remains static. An agent that repeatedly fails at a pattern never adjusts its approach unless a human manually edits its instructions.

## Industry context

| System | Approach | Cost per optimization | Production evidence |
|--------|----------|----------------------|-------------------|
| **LangMem gradient** | LLM reflection loop (think/critique/recommend) | 2-10 LLM calls | SDK, LangGraph integration |
| **DSPy MIPRO** | N candidates + Bayesian optimization | ~$2/run | +13% accuracy on multi-hop QA |
| **Letta sleep-time** | Background agents reorganize memory blocks | 1-3 LLM calls | Production (Letta Cloud) |
| **TextGrad** | LLM-as-judge textual gradients | 3-5 LLM calls | Published in Nature |
| **Comet/Opik** | Generate variants + A/B test | Variable | Enterprise production |

---

## Architecture

```
ConsolidationWorker (background, configurable interval)
    │
    ├── Phase 1: Importance decay (existing)
    ├── Phase 2: Garbage collection (existing)
    └── Phase 3: Prompt Optimization (NEW)
            │
            ▼
    ┌─────────────────────────────────┐
    │       PromptOptimizer           │
    │                                 │
    │  1. Collect reflections since   │
    │     last optimization           │
    │  2. Cluster by outcome          │
    │  3. LLM: analyze patterns →     │
    │     propose instruction changes │
    │  4. Apply to core memory blocks │
    │  5. Version + log decision      │
    └─────────────────────────────────┘
            │
            ▼
    ┌────────────────────────────────┐
    │  Core Memory Blocks            │
    │  (always in every prompt)      │
    │                                │
    │  persona: behavior patterns    │
    │  domain: learned expertise     │
    │  task_state: active context    │
    │  user_knowledge: preferences   │
    └────────────────────────────────┘
```

## 3-phase optimization cycle

### Phase 1: Collect & Cluster

Query SurrealDB for reflections since last optimization:
```sql
SELECT * FROM reflection
WHERE agent_id = $agent AND created_at > $since
ORDER BY created_at DESC LIMIT 50
```

Skip if < 3 reflections (insufficient data for pattern detection).

Cluster by outcome:
- **Failures**: what systematically doesn't work
- **Successes**: what consistently works well
- **Repeated lessons**: themes that appear in multiple reflections

### Phase 2: LLM Analysis (gradient approach)

Single LLM call (temperature 0.2) with structured JSON output:

```
SYSTEM: You are a prompt optimization engine for an AI agent. Analyze
the agent's reflections from recent tasks and propose specific, targeted
changes to the agent's instruction blocks.

Current instruction blocks:
<persona>{current_persona}</persona>
<domain>{current_domain}</domain>

Recent reflections (newest first):
{formatted_reflections}

Respond ONLY with valid JSON:
{
  "analysis": "2-3 sentence summary of patterns found",
  "changes": [
    {
      "block": "domain|persona|task_state",
      "action": "append|replace",
      "content": "specific instruction to add or replace with",
      "reason": "why this helps, citing specific reflections"
    }
  ],
  "no_change_reason": "reason if no changes needed, else null"
}

Rules:
- Only propose changes supported by 2+ reflections (not one-off events)
- Prefer append over replace (less destructive)
- Keep each change to 1-2 sentences
- Maximum 3 changes per cycle
- Never remove working instructions, only add/refine
- Focus on actionable behavior changes, not abstract principles
```

### Phase 3: Apply & Version

For each proposed change:
1. Snapshot current block content (for rollback)
2. Apply via `memory.update_core_block()` (replace) or `memory.append_core_block()` (append)
3. Store optimization record in memory:
   ```
   key: prompt_opt_{uuid}
   category: Custom("prompt_optimization")
   content: JSON { changes, analysis, previous_blocks, reflections_analyzed }
   ```
4. Log with `prompt.optimization.*` prefix

---

## Implementation

### New files

| File | Purpose |
|------|---------|
| `crates/adapters/core/src/memory_adapters/prompt_optimizer.rs` | PromptOptimizer: collect, analyze, apply |

### Modified files

| File | Change |
|------|--------|
| `crates/adapters/core/src/memory_adapters/consolidation_worker.rs` | Add Phase 3, accept Provider, configurable interval |
| `crates/adapters/core/src/memory_adapters/mod.rs` | Register new module |
| `crates/adapters/core/src/daemon/mod.rs` | Pass provider to consolidation worker |

### Types

```rust
/// Result of one optimization cycle.
pub struct PromptOptimization {
    pub id: String,
    pub agent_id: String,
    pub timestamp: DateTime<Utc>,
    pub reflections_analyzed: usize,
    pub changes: Vec<PromptChange>,
    pub analysis: String,
    pub previous_blocks: HashMap<String, String>, // snapshot for rollback
}

pub struct PromptChange {
    pub block: String,     // "domain", "persona", "task_state"
    pub action: String,    // "append", "replace"
    pub content: String,
    pub reason: String,
}
```

### Config

```rust
pub struct ConsolidationWorkerConfig {
    pub interval: Duration,                        // 1h (existing)
    pub gc_importance_threshold: f32,              // 0.05 (existing)
    pub gc_max_age_days: u32,                      // 30 (existing)
    pub optimization_interval: Duration,           // 6h (NEW)
    pub min_reflections_for_optimization: usize,   // 3 (NEW)
}
```

### Logging

| Log line | Level | When |
|----------|-------|------|
| `prompt.optimization.start` | INFO | Cycle begins, shows reflection count |
| `prompt.optimization.analysis` | INFO | LLM analysis summary |
| `prompt.optimization.change` | INFO | Each applied change (block, action, reason) |
| `prompt.optimization.applied` | INFO | Cycle complete, total changes |
| `prompt.optimization.skip` | DEBUG | Insufficient reflections |
| `prompt.optimization.failed` | WARN | LLM call or application error |

### Monitoring

After deploy:
```bash
journalctl --user -u synapseclaw.service | grep "prompt.optimization"
```

---

## Slices

| Slice | Description |
|-------|-------------|
| 1 | `prompt_optimizer.rs` — core logic (collect, LLM analyze, parse response) |
| 2 | Wire into ConsolidationWorker as Phase 3 + pass Provider |
| 3 | Logging + CLI command `synapseclaw memory optimize` for manual trigger |

---

## Acceptance criteria

| # | Criterion |
|---|-----------|
| 1 | After 3+ reflections, optimizer proposes changes based on patterns |
| 2 | Changes applied to core memory blocks (visible in next prompt) |
| 3 | Optimization record stored with snapshot for potential rollback |
| 4 | All decisions logged with `prompt.optimization.*` prefix |
| 5 | Insufficient data → skip (not error) |
| 6 | LLM failure → warn + skip (not crash) |

---

## Future (Phase 4.4b)

- A/B testing of prompt variants (needs eval framework + metric function)
- Automatic rollback on performance degradation (failure_rate tracking)
- Cross-agent prompt sharing via IPC MemoryEvent
- DSPy-style Bayesian optimization with trainset
- Multi-prompt credit assignment (which agent's prompt needs fixing)
