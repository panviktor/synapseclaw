# Skills Methods

Use this page when you know what you want to do but not which command or API path to use. Read-only methods inspect state; mutating methods create, update, apply, or roll back skills.

## Teach The Agent A Workflow

Use this when a human wants to create a reusable procedure.

Read-only check:

```bash
synapseclaw skills authored
```

Mutating create:

```bash
synapseclaw skills create --name <name> --body <markdown>
```

Runtime command:

```text
/skills create <name> :: <markdown>
```

API:

- `GET /api/skills/authored`
- `POST /api/skills/create`

## Inspect Generated Ideas

Use this when the system has proposed new skills or patches.

Read-only:

```bash
synapseclaw skills candidates
```

Runtime command:

```text
/skills candidates
```

API:

- `GET /api/skills/candidates`

## Review A Patch

Use this before changing an existing skill.

Read-only:

```bash
synapseclaw skills diff <candidate-id>
synapseclaw skills test <candidate-id>
```

Mutating:

```bash
synapseclaw skills apply <candidate-id>
```

API:

- `POST /api/skills/candidates/diff`
- `POST /api/skills/candidates/test`
- `POST /api/skills/candidates/apply`

Safety rule: apply must match the current target version and pass replay/eval gates.

## Undo A Skill Change

Use this when an applied patch or manual update made a skill worse.

Read-only:

```bash
synapseclaw skills versions <skill-id-or-name>
```

Mutating:

```bash
synapseclaw skills rollback <apply-record-or-ref>
```

API:

- `GET /api/skills/versions`
- `POST /api/skills/rollback`

Rollback is version-aware. It should refuse to restore an old snapshot when the target skill has changed since that snapshot was recorded.

## Measure Usefulness

Use this to understand whether skills are selected, read, helpful, failing, repaired, blocked, or rolled back.

Read-only:

```bash
synapseclaw skills health --limit 20 --trace-limit 100
synapseclaw skills traces --limit 50
```

Mutating cleanup:

```bash
synapseclaw skills health --apply
```

Runtime commands:

```text
/skills health
/skills traces
/skills health --apply
```

API:

- `GET /api/skills/health`
- `GET /api/skills/traces`
- `POST /api/skills/health/apply`

## Export Or Scaffold A Package

Use this when you need an editable file-backed package shape.

Mutating:

```bash
synapseclaw skills export <id-or-name>
synapseclaw skills scaffold <name>
```

API:

- `POST /api/skills/export`

Use export for an existing memory-backed skill. Use scaffold for a new package skeleton with `SKILL.md`, `references/`, `templates/`, and `assets/`.

