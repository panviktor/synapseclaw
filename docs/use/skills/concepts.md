# Skills Concepts

Skills are governed runtime capabilities. A skill has content, metadata, lifecycle state, origin, provenance, and utility signals.

## States

- `active`: the skill is available for use.
- `candidate`: the skill exists but needs review or promotion.
- `deprecated`: the skill is kept for history, rollback, or audit, but should not normally be selected.

## Origins

- `manual` or user-authored skills are created by a human.
- `learned` or generated skills are created or updated by the system.
- `package` or imported skills come from file-backed packages or open-skills sources.

## Compact Catalogs

The runtime should not put every full skill body into every model request. It first exposes compact catalog cards with identifiers, names, descriptions, status, tags, task family, and tool hints.

When the model needs the full instructions, it uses `skill_read`. Repeated activation is deduplicated, and old completed activations are represented by compact receipts rather than repeated bodies.

## Why This Matters

This design keeps skills useful without turning them into prompt bloat. It also gives operators a real lifecycle: create, audit, review, apply, rollback, measure health, and deprecate.

