# Memory Event Emission Research

Source artifact:

- `/tmp/phase_e_report.md`

## Why It Matters

The report describes a real gap between:

- memory-domain events
- IPC broadcast
- SSE/web observability
- gateway read models

This is useful because it explains why memory changes can exist in the system
without becoming first-class operator-visible events.

## Most Useful Findings

- `MemoryEvent` already exists as a domain concept.
- Current IPC emission is narrow and historically focused on a small subset of
  memory events.
- SSE/event-stream broadcasting is centered on runtime/tool/LLM events, not on
  memory-domain state transitions.
- Gateway memory APIs are stronger at CRUD than at read-model/projection
  surfaces.

## What To Keep

- memory events should be promoted into the same observability pipeline as
  other runtime events
- web/operator UI needs memory read-model endpoints, not just raw entry CRUD
- event emission should be explicit and typed, not reconstructed indirectly

## What To Treat Carefully

- some file references and implementation details in the original report may be
  stale
- this note should not be treated as proof that current behavior is unchanged

## Best Fit

This should become a future slice for:

- memory observability
- learning read models
- SSE/operator surfaces

It is adjacent to Phase 4.9, but not part of the core self-learning loop
itself.
