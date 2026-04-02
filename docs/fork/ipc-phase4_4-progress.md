# IPC Phase 4.4 Progress

**Status**: COMPLETE (PR #226)

Phase 4.3: memory architecture (COMPLETE) | **Phase 4.4: prompt optimizer (COMPLETE)** | Phase 4.5: pipeline hardening

---

## Goal

Agents automatically improve their own instructions by analyzing accumulated reflections and applying targeted changes to core memory blocks via LLM-driven optimization cycle.

---

## Slices

| Slice | Description | Status | PRs |
|-------|-------------|--------|-----|
| 1 | prompt_optimizer.rs — collect reflections, LLM analyze, parse, apply | TODO | |
| 2 | Wire into ConsolidationWorker + pass Provider + config | TODO | |
| 3 | Logging + CLI manual trigger | TODO | |

---

## Checklist

### prompt_optimizer.rs

| Item | Status |
|------|--------|
| `optimize_prompt()` function — collect, analyze, apply | TODO |
| OPTIMIZATION_PROMPT — structured JSON extraction | TODO |
| `PromptOptimization` + `PromptChange` types | TODO |
| Snapshot current blocks before applying (rollback support) | TODO |
| Store optimization record in Custom("prompt_optimization") | TODO |

### ConsolidationWorker integration

| Item | Status |
|------|--------|
| Accept `Provider` param in `spawn_consolidation_worker()` | TODO |
| Add Phase 3 after GC | TODO |
| Track `last_optimization` timestamp | TODO |
| Config: `optimization_interval` (default 6h) | TODO |
| Config: `min_reflections_for_optimization` (default 3) | TODO |

### Daemon wiring

| Item | Status |
|------|--------|
| Pass provider to consolidation worker in daemon/mod.rs | TODO |

### Logging

| Item | Status |
|------|--------|
| `prompt.optimization.start` | TODO |
| `prompt.optimization.analysis` | TODO |
| `prompt.optimization.change` | TODO |
| `prompt.optimization.applied` | TODO |
| `prompt.optimization.skip` | TODO |
| `prompt.optimization.failed` | TODO |

### CLI

| Item | Status |
|------|--------|
| `synapseclaw memory optimize` manual trigger | TODO |

---

## Acceptance criteria

| # | Criterion | Status |
|---|-----------|--------|
| 1 | 3+ reflections → optimizer proposes pattern-based changes | TODO |
| 2 | Changes applied to core memory blocks | TODO |
| 3 | Optimization record stored with rollback snapshot | TODO |
| 4 | All decisions logged with prompt.optimization.* | TODO |
| 5 | Insufficient data → skip | TODO |
| 6 | LLM failure → warn + skip | TODO |
