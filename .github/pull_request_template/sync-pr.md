## Upstream Sync Summary

- Sync branch: `{{SYNC_BRANCH}}`
- Base branch: `main`
- Upstream range: `{{OLD_MAIN_SHA}}..{{UPSTREAM_SHA}}`
- Upstream HEAD: `{{UPSTREAM_SHA}}`
- Merge status: `clean`

## What changed

- [ ] reviewed upstream commit range
- [ ] reviewed shared-hotspot files
- [ ] checked whether any fork boundary moved
- [ ] checked whether any candidate-upstream primitive should be extracted

## Hotspot Review

List touched hotspot paths here:
- `src/config/schema.rs`
- `src/gateway/mod.rs`
- `src/security/pairing.rs`

Delete lines that do not apply and add missing ones.

## Fork Invariants

- [ ] IPC ACL invariants
- [ ] correlated `result` only
- [ ] legacy tokens = no IPC
- [ ] quarantine = read-only for execution
- [ ] approval routing via Opus / control plane
- [ ] revoke / disable / downgrade / quarantine semantics
- [ ] lateral messaging restrictions
- [ ] no bypass through channel auto-approve for IPC-originated dangerous actions

## Conflict Notes

- [ ] no conflicts occurred
- [ ] conflicts occurred and were manually resolved
- [ ] `fork-delta.md` updated if required

## Reviewer Focus

Please pay extra attention to:
- auth / trust semantics
- approval / quarantine boundaries
- config masking / secret handling
- behavior changes in shared-hotspot files
