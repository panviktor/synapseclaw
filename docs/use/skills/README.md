# Skills

A skill is a reusable procedure the agent can discover and load when it is relevant to the current task. Skills are not just prompt snippets: they have lifecycle state, audit, provenance, versioning, health signals, and a controlled activation path.

Synapseclaw currently supports four practical skill lanes:

- **User-authored skills** are written by a human in normal Markdown and stored in memory with governance metadata.
- **Generated new skills** are candidates created by the system from repeated useful patterns.
- **Generated patch candidates** are proposed improvements to existing skills.
- **Imported packages** are file-backed skill packages or open-skills sources indexed into the runtime catalog.

The runtime uses compact skill catalog entries first. Full skill bodies are loaded through `skill_read` only when a specific skill becomes relevant, which keeps provider context small.

## Start Here

- [Quickstart](quickstart.md) - create and activate a user skill.
- [Concepts](concepts.md) - states, origins, compact catalogs, and activation.
- [Create a user skill](create-user-skill.md) - write a skill by hand.
- [Generated skills](generated-skills.md) - understand generated candidates and patches.
- [Review, apply, and rollback](review-apply-rollback.md) - operate generated patches safely.
- [Import and export packages](import-export-packages.md) - work with file-backed packages.
- [Health and auto-promotion](health-and-autopromote.md) - inspect usefulness and guarded promotion.
- [Troubleshooting](troubleshooting.md) - common failures and checks.

