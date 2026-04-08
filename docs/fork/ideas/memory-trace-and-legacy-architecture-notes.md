# Memory Trace and Legacy Architecture Notes

Source artifacts:

- `/tmp/synapseclaw_memory_trace.md`
- `/tmp/synapseclaw-phase43-memory-architecture.md`
- `/tmp/synapseclaw_architecture.md`

## Why They Matter

These artifacts are valuable as historical context:

- how memory flows were previously understood
- how the broader SurrealDB-centered memory design was framed
- how tool registration / cron / delivery were previously documented

They are useful for orientation and archaeology.

## What Is Still Useful

- broad memory tier separation
- SurrealDB-centered storage ideas
- explicit knowledge-graph / semantic / episodic / skill distinctions
- cron and delivery wiring context
- historical end-to-end memory trace references

## What To Treat Carefully

These notes are not canonical current behavior.

Reasons:

- they describe earlier implementation stages
- some flows have already changed in Phase 4.6–4.8
- some assumptions were architectural proposals, not shipped contracts

## Recommended Use

Use these notes when you need:

- historical context
- an older design rationale
- a rough map of how memory and tools evolved

Do not treat them as the authority for current runtime behavior.

## Best Fit

This note is an archive reference, not an active execution plan.
