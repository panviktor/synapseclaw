---
name: Upstream Sync Conflict
about: Auto-created when upstream sync has merge conflicts
labels: upstream-sync, conflict
---

# Upstream Sync Conflict

## Summary

- Sync branch: `{{SYNC_BRANCH}}`
- Main branch: `main`
- Upstream HEAD: `{{UPSTREAM_SHA}}`
- Status: `conflict`

## Required actions

- [ ] review conflict report artifact
- [ ] assign hotspot owners
- [ ] resolve merge conflicts on `sync/...`
- [ ] rerun CI and fork invariants
- [ ] update `fork-delta.md` if fork boundary changed

## Review focus

- auth / trust changes
- pairing / token metadata changes
- gateway / approval / quarantine semantics
- scheduler or channel side effects
