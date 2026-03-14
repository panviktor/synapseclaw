---
name: sync-upstream
description: "Sync fork with upstream ZeroClaw repository. Fetches upstream/master, updates vendor branch, creates sync PR with conflict analysis and hotspot review. Use when the user says 'sync upstream', 'обнови upstream', 'pull from upstream', or wants to merge latest upstream changes."
user-invocable: true
---

# Upstream Sync

Sync the fork with upstream ZeroClaw. Follow the strategy documented in `docs/fork/sync-strategy.md`.

## Reference files (read before acting)

- `docs/fork/sync-strategy.md` — sync cadence, branch model, merge strategy
- `docs/fork/delta-registry.md` — fork-owned files, shared hotspots
- `docs/fork/sync-review-rubric.md` — what to check in sync PRs

## Step 1: Fetch upstream

```bash
git fetch upstream
git log --oneline upstream/master -5
git log --oneline origin/main -5
```

Check divergence:
```bash
git rev-list --count origin/main..upstream/master
git rev-list --count upstream/master..origin/main
```

If 0 commits behind, report "already up to date" and stop.

## Step 2: Update vendor branch

```bash
git checkout vendor/upstream-master
git merge upstream/master --ff-only
git push origin vendor/upstream-master
git checkout main
```

If ff-only fails, something is wrong with the vendor branch. Report and stop.

## Step 3: Create sync branch

```bash
SYNC_DATE=$(date +%Y%m%d)
git checkout -b sync/upstream-$SYNC_DATE main
```

## Step 4: Merge upstream

```bash
git merge upstream/master --no-edit
```

If conflicts:
1. List conflicted files: `git diff --name-only --diff-filter=U`
2. Check each against the delta registry — fork-owned files use ours-first strategy
3. For shared hotspots (`src/config/schema.rs`, `src/gateway/mod.rs`, etc.) — manual review required
4. Report conflicts to user with recommended resolution strategy per file
5. Do NOT auto-resolve — wait for user decision

If no conflicts, proceed.

## Step 5: Hotspot review

Even without conflicts, review these shared hotspots for semantic changes:

```
src/config/schema.rs
src/config/mod.rs
src/gateway/mod.rs
src/gateway/api.rs
src/security/pairing.rs
src/tools/mod.rs
src/onboard/wizard.rs
```

For each modified hotspot, show the diff and flag if fork-specific code (IPC, trust levels, token metadata) might be affected.

## Step 6: Validate

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test -q
```

Also run fork-invariant tests specifically:
```bash
cargo test gateway::ipc::tests
cargo test tools::agents_ipc::tests
cargo test security::pairing::tests
```

## Step 7: Push + PR

```bash
git push -u origin sync/upstream-$SYNC_DATE
```

Create PR via REST API:
```bash
gh api repos/panviktor/zeroclaw/pulls -X POST \
  -f title="chore(sync): merge upstream/master into fork ($SYNC_DATE)" \
  -f head="sync/upstream-$SYNC_DATE" \
  -f base="main" \
  -f body="$(cat <<'EOF'
## Summary
- Sync with upstream/master (<N> commits)
- Conflicts: <none | list>
- Hotspot changes: <none | list>

## Upstream changes
<list of notable upstream commits>

## Shared hotspot review
<per-file notes or "no changes to hotspots">

## Test plan
- [x] `cargo fmt` — clean
- [x] `cargo clippy` — clean
- [x] `cargo test` — <N> passed
- [x] Fork-invariant tests — passed

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

Return the PR URL.

## Arguments

- No args: full sync workflow
- `check`: only fetch + show divergence, no merge
- `conflicts`: show what would conflict without actually merging (dry run)
