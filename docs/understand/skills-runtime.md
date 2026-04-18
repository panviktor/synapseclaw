# Skills Runtime

The skills runtime treats a skill as a governed capability rather than a plain Markdown snippet. A skill can be memory-backed, generated, manually authored, package-backed, active, candidate, or deprecated.

The runtime builds compact catalog cards for discovery, then loads full bodies through `skill_read` only when needed. It records activation receipts, use traces, health counters, patch candidates, version snapshots, and rollback records without repeatedly placing full skill bodies into provider context.

## Runtime Dependencies

```mermaid
flowchart TD
    CLI[CLI skills commands] --> Gateway[Gateway Skills API]
    Web[Web Skills page] --> Gateway
    Runtime[/skills runtime commands] --> Executor[Shared skill command executor]

    Gateway --> Services[Skill services]
    Executor --> Services

    Services --> Memory[(Memory store)]
    Services --> Audit[Skill audit]
    Services --> Governance[Governance and lifecycle]
    Services --> Eval[Replay and eval gates]
    Services --> Health[Health service]

    Memory --> Embeddings[Embedding index]
    Embeddings --> Retrieval[Semantic retrieval]
    Retrieval --> Catalog[Compact skill catalog]
    Catalog --> SkillRead[skill_read]
    SkillRead --> Provider[Provider context]
```

CLI, web, and runtime commands should converge on the same skill services. They may differ in transport or presentation, but they should not implement separate lifecycle rules.

## Runtime Shape

- Storage keeps memory-backed skills, generated candidates, versions, and rollback records.
- Retrieval uses compact cards and embedding-backed search when available.
- Activation uses `skill_read` to load one governed full body.
- Traces record compact evidence without storing repeated full instructions.
- Health folds traces and lifecycle records into utility counters.
- Patch apply creates version history instead of overwriting without a trail.
