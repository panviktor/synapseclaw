# Skills

Skills turn repeated work into governed, reusable capability without bloating the model context. A skill is not a prompt snippet: it has lifecycle state, audit, provenance, versioning, health signals, and a controlled activation path.

Synapseclaw currently supports four practical skill lanes:

- **User-authored skills** are written by a human in normal Markdown and stored in memory with governance metadata.
- **Generated new skills** are candidates created by the system from repeated useful patterns.
- **Generated patch candidates** are proposed improvements to existing skills.
- **Imported packages** are file-backed skill packages or open-skills sources indexed into the runtime catalog.

The runtime uses compact skill catalog entries first. Full skill bodies are loaded through `skill_read` only when a specific skill becomes relevant, which keeps provider context small and makes skill use measurable.

## Start Here

- [Quickstart](quickstart.md) - create and activate a user skill.
- [Mental model](mental-model.md) - understand what a skill is and why it is governed.
- [Concepts](concepts.md) - states, origins, compact catalogs, and activation.
- [Methods](methods.md) - choose the right command or API by intent.
- [Create a user skill](create-user-skill.md) - write a skill by hand.
- [Authoring patterns](authoring-patterns.md) - templates, good examples, and anti-patterns.
- [On-demand loading](on-demand-loading.md) - see how compact cards and `skill_read` protect context.
- [Why skills work this way](why-skills-work-this-way.md) - design rationale and product philosophy.
- [Safety and governance](safety-and-governance.md) - audit, review, policy, and no-secrets rules.
- [Generated skills](generated-skills.md) - understand generated candidates and patches.
- [Review, apply, and rollback](review-apply-rollback.md) - operate generated patches safely.
- [Import and export packages](import-export-packages.md) - work with file-backed packages.
- [Health and auto-promotion](health-and-autopromote.md) - inspect usefulness and guarded promotion.
- [Troubleshooting](troubleshooting.md) - common failures and checks.
