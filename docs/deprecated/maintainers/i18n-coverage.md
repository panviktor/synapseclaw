# SynapseClaw i18n Coverage and Structure

This document defines the localization structure for SynapseClaw docs and tracks current coverage.

Last refreshed: **April 3, 2026**.

## Canonical Layout

Use these i18n paths:

- Canonical docs entry points are English: `README.md`, `docs/README.md`, `docs/SUMMARY.md`
- Compatibility translations may live beside the English source as `*.vi.md` when only a small operator subset is maintained
- A full `docs/i18n/<locale>/...` tree should be introduced only when that locale has an intentionally maintained hub and TOC

## Locale Coverage Matrix

| Locale | Root README | Canonical Docs Hub | Commands Ref | Config Ref | Troubleshooting | Status |
|---|---|---|---|---|---|---|
| `en` | `README.md` | `docs/README.md` | `docs/reference/cli/commands-reference.md` | `docs/reference/api/config-reference.md` | `docs/ops/troubleshooting.md` | Source of truth |
| `vi` | - | - | `docs/reference/cli/commands-reference.vi.md` | `docs/reference/api/config-reference.vi.md` | `docs/ops/troubleshooting.vi.md` | Selected compatibility pages only |

Additional compatibility translation:

- `docs/setup-guides/one-click-bootstrap.vi.md`

No localized README or docs hub is currently maintained.

## Collection Index i18n

Collection index docs are maintained in English only. Compatibility translations currently cover selected operator references, not the collection indexes.

## Localization Rules

- Keep technical identifiers in English:
  - CLI command names
  - config keys
  - API paths
  - trait/type identifiers
- Prefer concise, operator-oriented localization over literal translation.
- Update "Last refreshed" dates when compatibility pages change.
- Do not advertise a localized hub or TOC unless those pages actually exist and are maintained.

## Adding a New Locale

1. Create `README.<locale>.md`.
2. Create canonical docs tree under `docs/i18n/<locale>/` only if the locale will have a maintained hub and TOC.
3. Add locale links to:
   - root language nav in `README.md`
   - docs hub status line in `docs/README.md`
   - language entry section in `docs/SUMMARY.md`
4. Add any compatibility translation pages only after the canonical English doc is stable.
5. Update this file (`docs/i18n-coverage.md`) and run link validation.

## Review Checklist

- Links resolve for every localized or compatibility page that is advertised.
- No docs claim a localized README or hub that does not exist.
- TOC (`docs/SUMMARY.md`) and docs hub (`docs/README.md`) match the actual i18n footprint.
