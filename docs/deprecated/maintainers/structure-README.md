# SynapseClaw Docs Structure Map

This page defines the documentation structure across three axes:

1. Language
2. Part (category)
3. Function (document intent)

Last refreshed: **April 3, 2026**.

## 1) By Language

| Language | Entry point | Canonical tree | Notes |
|---|---|---|---|
| English | `docs/README.md` | `docs/` | Source-of-truth runtime behavior docs are authored in English first. |
| Vietnamese compatibility pages (`vi`) | selected `*.vi.md` files alongside English sources | same directories as English docs | Partial operator-facing translations only; no localized hub or TOC is currently maintained. |

## 2) By Part (Category)

These directories are the primary navigation modules by product area.

- `docs/setup-guides/` for initial setup and first-run flows
- `docs/reference/` for command/config/provider/channel reference indexes
- `docs/ops/` for day-2 operations, deployment, and troubleshooting entry points
- `docs/security/` for security guidance and security-oriented navigation
- `docs/contributing/` for contribution and CI/review processes
- `docs/maintainers/` for project snapshots, inventory, and status-oriented docs
- `docs/fork/` for fork-specific architecture, IPC plans, and progress tracking
- `docs/i18n/` for localization status and future coordination notes
- `docs/assets/` and `docs/superpowers/` for supporting assets and narrow-scope specs

## 3) By Function (Document Intent)

Use this grouping to decide where new docs belong.

### Runtime Contract (current behavior)

- `docs/reference/cli/commands-reference.md`
- `docs/reference/api/providers-reference.md`
- `docs/reference/api/channels-reference.md`
- `docs/reference/api/config-reference.md`
- `docs/ops/operations-runbook.md`
- `docs/ops/troubleshooting.md`

### Setup / Integration Guides

- `docs/setup-guides/one-click-bootstrap.md`
- `docs/setup-guides/macos-update-uninstall.md`
- `docs/setup-guides/zai-glm-setup.md`
- `docs/setup-guides/nextcloud-talk-setup.md`
- `docs/setup-guides/mattermost-setup.md`
- `docs/security/matrix-e2ee-guide.md`
- `docs/contributing/custom-providers.md`
- `docs/contributing/langgraph-integration.md`

### Policy / Process

- `docs/contributing/pr-workflow.md`
- `docs/contributing/reviewer-playbook.md`
- `docs/contributing/ci-map.md`
- `docs/contributing/actions-source-policy.md`
- `docs/contributing/docs-contract.md`
- `docs/contributing/pr-discipline.md`

### Proposals / Roadmaps

- `docs/security/sandboxing.md`
- `docs/ops/resource-limits.md`
- `docs/security/audit-logging.md`
- `docs/security/agnostic-security.md`
- `docs/security/frictionless-security.md`
- `docs/security/security-roadmap.md`
- `docs/fork/ipc-plan.md`

### Snapshots / Time-Bound Reports

- `docs/maintainers/project-triage-snapshot-2026-02-18.md`

### Fork Planning / Execution

- `docs/fork/delta-registry.md`
- `docs/fork/ipc-quickstart.md`
- `docs/fork/ipc-progress.md`

### Assets / Templates

- `docs/assets/`
- `docs/contributing/doc-template.md`

## Placement Rules (Quick)

- New runtime behavior docs must be linked from the appropriate category index and `docs/SUMMARY.md`.
- English docs are canonical unless a localized hub is intentionally reintroduced.
- If a localized compatibility page is added or restored, record its scope in `docs/maintainers/i18n-coverage.md`.
- Keep fork plans in `docs/fork/` separate from runtime-contract references to avoid mixing roadmap text into operator docs.
