# Memory And Compaction

Memory and compaction preserve useful continuity while keeping provider context small. Stable facts, compact traces, and activation receipts can survive context pressure without forcing every historical detail into every model request.

Skills are the clearest progressive-loading example. The runtime can remember that a skill exists and when it was activated, while avoiding repeated full instruction bodies after the immediate tool cycle.

Compaction summaries and vector embeddings use explicit [model lanes](../reference/model-lanes.md). Missing lanes degrade visibly instead of borrowing the primary model silently, which keeps token spending and model behavior inspectable.

Implicit memory recall now follows the same discipline. The runtime can prepare a bounded recall hypothesis before a turn, but that plan is derived from typed turn interpretation and runtime state instead of phrase-matching prompt hacks. Mutable or stale facts still need verification before final action.

Provider history and audit history stay separate. The model sees only compact continuity material, while richer repair traces, watchdog alerts, usage ledgers, and replay artifacts remain available for diagnostics without bloating every request.
