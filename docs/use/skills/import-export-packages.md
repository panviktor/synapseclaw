# Import And Export Skill Packages

Older skill packages may exist as `SKILL.md` or `SKILL.*` files in the workspace. Local workspace packages are ported into memory and moved under `workspace/skills/ported/`; imported or open-skills packages remain file-backed but get compact semantic index cards.

This transition lets the runtime find package skills semantically without putting every full package body into provider context.

## Export A Memory-Backed Skill

```bash
synapseclaw skills export <id-or-name>
```

Export writes an editable package under `workspace/skills` and preserves source id, origin, status, task family, tool hints, and tags in package metadata.

## Scaffold A Package

```bash
synapseclaw skills scaffold <name>
```

Scaffold creates a safe editable package shape with `SKILL.md`, `references/`, `templates/`, and `assets/`. The package should pass audit before it is considered usable.

## Package Locations

- `workspace/skills/` contains editable local packages.
- `workspace/skills/ported/` contains migrated legacy packages.
- `references/`, `templates/`, and `assets/` hold supporting package material.

