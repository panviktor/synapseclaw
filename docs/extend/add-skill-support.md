# Add Skill Support

Skills are stored, indexed, audited, selected, activated, measured, and versioned through the runtime. New skill behavior should reuse the existing lifecycle instead of creating a separate web-only or channel-only path.

The correct loading model is compact catalog, then `skill_read`, then compact activation receipt. Do not inline full skill bodies directly into provider context as a shortcut.

## Developer Model

Skill support has six cooperating parts:

- Storage owns memory-backed skills, generated candidates, versions, rollback records, and compact package index cards.
- Retrieval finds compact skill cards, using embeddings when an embedding profile is active.
- Governance decides whether a skill is active, candidate, deprecated, shadowed, or blocked.
- Activation loads one full body through `skill_read`.
- Trace and health services record compact utility evidence.
- Patch apply and rollback preserve a version trail.

## Integration Rules

Use the shared skill command executor and gateway API for web, channel, and runtime command behavior. Do not create a separate lifecycle branch just because the caller is web UI, CLI, or a channel message.

When adding new skill behavior, decide whether it is read-only or mutating. Read-only paths can inspect catalog, health, versions, candidates, and diffs. Mutating paths must go through audit, governance, versioning, and policy checks.

## What Not To Do

- Do not inline full skill bodies into bootstrap prompts.
- Do not let generated skills become active without gates.
- Do not store secrets in skill bodies.
- Do not add string-based success/failure parsing when typed traces exist.
- Do not make imported packages mutable through memory-owned update paths.
- Do not duplicate `/skills` command logic separately for web and channels.
