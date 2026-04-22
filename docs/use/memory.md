# Memory

Memory lets the runtime keep useful facts, traces, and procedural knowledge without putting everything into every model request. The goal is better continuity with smaller provider-facing context.

Session, project, and memory replay stay limited until typed privacy classification is complete. Skills are the stable procedural-memory path: the runtime finds compact skill cards first and loads full instructions only when needed.

Vector recall depends on an explicit `embedding` model lane. If embeddings are not configured, memory still works without vector search, and `synapseclaw doctor` reports the lane status.

For normal chat turns, SynapseClaw can also prepare an implicit memory recall hint before tool use. That recall is bounded, redacted in diagnostics, and treated as a hypothesis rather than a silent rewrite of user intent. If the recalled fact looks stale or conflicting, the runtime should verify it before acting on it.
