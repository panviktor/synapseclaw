# Skills Runtime

The skills runtime treats a skill as a governed capability rather than a plain Markdown snippet. A skill can be memory-backed, generated, manually authored, package-backed, active, candidate, or deprecated.

The runtime builds compact catalog cards for discovery, then loads full bodies through `skill_read` only when needed. It records activation receipts, use traces, health counters, patch candidates, version snapshots, and rollback records without repeatedly placing full skill bodies into provider context.

