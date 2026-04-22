# IPC Phase 4.5 Progress

**Status**: NOT STARTED

Phase 4.4: prompt optimizer (DONE) | **Phase 4.5: pipeline hardening** | Phase 4.6: TBD

---

## Goal

Close remaining pipeline gaps: per-agent message filtering (AutoGen pattern), dead letter queue for failed steps, and pipeline visualization (ASCII + Mermaid + web).

---

## Slices

| Slice | Description | Status | PRs |
|-------|-------------|--------|-----|
| 1 | MessageFilterAgent — per-agent inbox filtering | TODO | |
| 2 | Dead Letter Queue — failed steps queued for retry/inspection | TODO | |
| 3 | Pipeline Visualization — ASCII + Mermaid rendering | TODO | |
| 4 | Web Dashboard — DLQ tab + pipeline graph page | TODO | |

---

## Checklist

### Slice 1: MessageFilterAgent

| Item | Status |
|------|--------|
| InboxFilterConfig in schema.rs | TODO |
| Filter logic in IPC inbox handler | TODO |
| agents_inbox tool integration | TODO |
| Agent config.toml updates | TODO |

### Slice 2: Dead Letter Queue

| Item | Status |
|------|--------|
| DeadLetter domain type + DeadLetterStatus | TODO |
| DeadLetterPort trait | TODO |
| Pipeline service: enqueue on step failure | TODO |
| Storage adapter (SQLite/SurrealDB) | TODO |
| REST API: list, retry, dismiss | TODO |
| CLI: pipeline dead-letters, retry, dismiss | TODO |

### Slice 3: Pipeline Visualization

| Item | Status |
|------|--------|
| PipelineDefinition::to_ascii() | TODO |
| PipelineDefinition::to_mermaid() | TODO |
| CLI: pipeline show \<name\> | TODO |
| REST API: /api/pipelines/:name/graph | TODO |

### Slice 4: Web Dashboard

| Item | Status |
|------|--------|
| Dead Letters tab | TODO |
| Pipeline graph page (mermaid.js) | TODO |
| Step status colors | TODO |

---

## Acceptance criteria

| # | Criterion | Status |
|---|-----------|--------|
| 1 | Per-agent inbox filtering works with per_source + allowed_kinds | TODO |
| 2 | Failed pipeline steps enqueued in DLQ | TODO |
| 3 | DLQ retry re-executes step with original input | TODO |
| 4 | pipeline show renders ASCII graph | TODO |
| 5 | Web dashboard renders Mermaid pipeline graph | TODO |
| 6 | FanOut/FanIn and conditional branches rendered correctly | TODO |
