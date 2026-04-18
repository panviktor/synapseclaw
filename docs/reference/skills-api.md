# Skills API Reference

The Skills API is the gateway surface for listing, creating, reviewing, applying, rolling back, and measuring skills. Request and response shapes may grow, but endpoint purpose should remain stable.

## Listing And Authoring

- `GET /api/skills/learned` lists learned/runtime-generated skills owned by the agent.
- `GET /api/skills/authored` lists memory-backed user-authored skills owned by the agent.
- `POST /api/skills/create` creates a memory-backed user-authored skill.
- `POST /api/skills/update` updates a memory-backed manual or learned skill.
- `POST /api/skills/export` exports a memory-backed skill as a workspace package.

## Candidates And Review

- `GET /api/skills/candidates` lists learned skill candidates and generated patch candidates.
- `POST /api/skills/candidates/diff` returns a compact diff/review view for a patch candidate.
- `POST /api/skills/candidates/test` runs replay/eval checks for a patch candidate.
- `POST /api/skills/candidates/apply` applies a tested patch candidate to its target skill.
- `GET /api/skills/review` returns deterministic learned skill review decisions as a dry run.
- `POST /api/skills/review/apply` applies deterministic learned skill review decisions.

## Versions And Rollback

- `GET /api/skills/versions` shows applied patch and rollback records.
- `POST /api/skills/rollback` rolls back an applied patch or update using its saved snapshot.

## Health And Policy

- `GET /api/skills/traces` lists compact skill use traces.
- `GET /api/skills/health` returns read-only skill catalog health and cleanup guidance.
- `POST /api/skills/health/apply` applies eligible learned-skill cleanup changes.
- `GET /api/skills/autopromote` evaluates generated patch auto-promotion policy as a dry run.
- `POST /api/skills/autopromote/apply` applies eligible generated patches if policy is enabled.

## Status

- `POST /api/skills/status` sets a learned or local memory skill status.
- `POST /api/skills/promote` marks a skill active.
- `POST /api/skills/demote` marks a skill candidate.
- `POST /api/skills/reject` marks a skill deprecated.

Common failures include unknown skill id, stale patch target version, failed replay/eval, failed audit, policy rejection, and attempts to mutate imported packages that are not owned by memory storage.

