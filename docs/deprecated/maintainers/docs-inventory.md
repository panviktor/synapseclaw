# SynapseClaw Documentation Inventory

This inventory classifies docs by intent so readers can quickly distinguish runtime-contract guides from design proposals.

Last reviewed: **April 3, 2026**.

## Classification Legend

- **Current Guide/Reference**: intended to match current runtime behavior
- **Policy/Process**: collaboration or governance rules
- **Proposal/Roadmap**: design exploration; may include hypothetical commands
- **Snapshot**: time-bound operational report

## Documentation Entry Points

| Doc | Type | Audience |
|---|---|---|
| `README.md` | Current Guide | all readers |
| `docs/README.md` | Current Guide (hub) | all readers |
| `docs/SUMMARY.md` | Current Guide (unified TOC) | all readers |
| `docs/maintainers/structure-README.md` | Current Guide (structure map) | maintainers |
| `docs/i18n/README.md` | Current Guide (i18n status) | maintainers/translators |

English is the only maintained docs entry point. Selected Vietnamese compatibility pages are tracked separately below.

## Collection Index Docs

| Doc | Type | Audience |
|---|---|---|
| `docs/setup-guides/README.md` | Current Guide | new users |
| `docs/reference/README.md` | Current Guide | users/operators |
| `docs/ops/README.md` | Current Guide | operators |
| `docs/security/README.md` | Current Guide | operators/contributors |
| `docs/contributing/README.md` | Current Guide | contributors/reviewers |
| `docs/maintainers/README.md` | Current Guide | maintainers |
| `docs/fork/README.md` | Current Guide | contributors/operators |

## Current Guides & References

| Doc | Type | Audience |
|---|---|---|
| `docs/setup-guides/one-click-bootstrap.md` | Current Guide | users/operators |
| `docs/setup-guides/macos-update-uninstall.md` | Current Guide | macOS users |
| `docs/setup-guides/nextcloud-talk-setup.md` | Current Guide | operators |
| `docs/setup-guides/mattermost-setup.md` | Current Guide | operators |
| `docs/setup-guides/zai-glm-setup.md` | Current Provider Setup Guide | users/operators |
| `docs/reference/cli/commands-reference.md` | Current Reference | users/operators |
| `docs/reference/api/providers-reference.md` | Current Reference | users/operators |
| `docs/reference/api/channels-reference.md` | Current Reference | users/operators |
| `docs/reference/api/config-reference.md` | Current Reference | operators |
| `docs/contributing/custom-providers.md` | Current Integration Guide | integration developers |
| `docs/contributing/langgraph-integration.md` | Current Integration Guide | integration developers |
| `docs/ops/operations-runbook.md` | Current Guide | operators |
| `docs/ops/troubleshooting.md` | Current Guide | users/operators |
| `docs/ops/network-deployment.md` | Current Guide | operators |
| `docs/security/matrix-e2ee-guide.md` | Current Guide | Matrix operators |
| `docs/fork/ipc-quickstart.md` | Current Guide | fork operators/contributors |

## Localized Compatibility Pages

| Doc | Type | Audience |
|---|---|---|
| `docs/setup-guides/one-click-bootstrap.vi.md` | Compatibility translation | Vietnamese users/operators |
| `docs/reference/cli/commands-reference.vi.md` | Compatibility translation | Vietnamese operators |
| `docs/reference/api/config-reference.vi.md` | Compatibility translation | Vietnamese operators |
| `docs/ops/troubleshooting.vi.md` | Compatibility translation | Vietnamese operators |

## Policy / Process Docs

| Doc | Type |
|---|---|
| `docs/contributing/pr-workflow.md` | Policy |
| `docs/contributing/reviewer-playbook.md` | Process |
| `docs/contributing/ci-map.md` | Process |
| `docs/contributing/actions-source-policy.md` | Policy |
| `docs/contributing/docs-contract.md` | Policy |
| `docs/contributing/pr-discipline.md` | Policy |
| `docs/contributing/change-playbooks.md` | Process |

## Proposal / Roadmap Docs

These are valuable context, but **not strict runtime contracts**.

| Doc | Type |
|---|---|
| `docs/security/sandboxing.md` | Proposal |
| `docs/ops/resource-limits.md` | Proposal |
| `docs/security/audit-logging.md` | Proposal |
| `docs/security/agnostic-security.md` | Proposal |
| `docs/security/frictionless-security.md` | Proposal |
| `docs/security/security-roadmap.md` | Roadmap |
| `docs/fork/ipc-plan.md` | Roadmap |
| `docs/fork/delta-registry.md` | Current architecture delta registry |

## Snapshot Docs

| Doc | Type |
|---|---|
| `docs/maintainers/project-triage-snapshot-2026-02-18.md` | Snapshot |

## Maintenance Recommendations

1. Update `docs/reference/cli/commands-reference.md` whenever CLI surface changes.
2. Update `docs/reference/api/providers-reference.md` when provider catalog/aliases/env vars change.
3. Update `docs/reference/api/channels-reference.md` when channel support or allowlist semantics change.
4. Keep snapshots date-stamped and immutable.
5. Mark proposal docs clearly to avoid being mistaken for runtime contracts.
6. Treat English docs as canonical until a localized hub is intentionally restored.
7. Update `docs/SUMMARY.md` and collection indexes whenever new major docs are added.
8. Track any localized compatibility pages in `docs/maintainers/i18n-coverage.md`.
