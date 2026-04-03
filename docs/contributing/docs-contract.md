# Documentation System Contract

Treat documentation as a first-class product surface, not a post-merge artifact.

## Canonical Entry Points

- root README: `README.md`
- docs hub: `docs/README.md`
- unified TOC: `docs/SUMMARY.md`

## Localization Status

- English is the only maintained docs entry point.
- Selected Vietnamese compatibility pages may exist for specific operator references, but they do not define the docs IA.

## Collection Indexes

- `docs/setup-guides/README.md`
- `docs/reference/README.md`
- `docs/ops/README.md`
- `docs/security/README.md`
- `docs/contributing/README.md`
- `docs/maintainers/README.md`

## Governance Rules

- Keep README/hub top navigation and quick routes intuitive and non-duplicative.
- Treat English docs as the source of truth for current behavior and navigation.
- If a localized page is added or restored, update navigation pointers and record its scope in `docs/maintainers/i18n-coverage.md` in the same PR.
- Keep proposal/roadmap docs explicitly labeled; avoid mixing proposal text into runtime-contract docs.
- Keep project snapshots date-stamped and immutable once superseded by a newer date.
