---
name: sync-upstream
description: "Sync fork with upstream SynapseClaw repository. Fetches upstream/master, updates vendor branch, creates sync PR with conflict analysis and hotspot review. Use when the user says 'sync upstream', 'обнови upstream', 'pull from upstream', or wants to merge latest upstream changes."
user-invocable: true
---

# Upstream Sync

Sync the fork with upstream SynapseClaw. Follow the strategy documented in `docs/fork/sync-strategy.md`.

## Reference files (read before acting)

- `docs/fork/sync-strategy.md` — sync cadence, branch model, merge strategy
- `docs/fork/delta-registry.md` — fork-owned files, shared hotspots, 44+ delta entries
- `docs/fork/sync-review-rubric.md` — what to check in sync PRs

## Sync State

Last full sync: 2026-03-17 (PRs #119, #121). Fork is synced with upstream v0.4.3 + 18 post-release commits.
`git rerere` has recorded resolutions for all known conflict patterns.

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

## Step 2: Assess scope

If ≤20 commits behind: cherry-pick batch (low risk).
If >20 commits behind: full merge preferred (rerere will handle known patterns).

For any scope, first do a dry-run:
```bash
git merge-tree --write-tree origin/main upstream/master 2>&1 | grep CONFLICT
```

## Step 3: Update vendor branch

```bash
git checkout vendor/upstream-master
git merge upstream/master --ff-only
git push origin vendor/upstream-master
git checkout main
```

If ff-only fails, something is wrong with the vendor branch. Report and stop.

## Step 4: Create sync branch

```bash
SYNC_DATE=$(date +%Y%m%d)
git checkout -b sync/upstream-$SYNC_DATE main
```

## Step 5: Merge or cherry-pick

### Full merge (preferred for >20 commits)

```bash
git merge --no-ff --no-commit vendor/upstream-master
```

### Cherry-pick (for ≤20 commits)

```bash
git cherry-pick --no-commit <hash1> <hash2> ...
```

## Step 6: Resolve conflicts

### Known conflict patterns (from 2026-03-17 sync)

**`src/config/mod.rs`** — re-export list. Accept upstream, re-add our types: `AgentsIpcConfig`, `IpcPromptGuardConfig`, `TokenMetadata`, STT types.

**`src/config/schema.rs`** — Config struct fields + secret decrypt/encrypt. Accept upstream new fields, preserve our `agents_ipc` field and IPC structs block (between GatewayConfig and ComposioConfig). Keep BOTH matrix secret + STT secret decrypt/encrypt blocks.

**`src/tools/mod.rs`** — tool modules + registration. Accept upstream new tools, preserve our 3-tuple return: `(boxed_registry, delegate_handle, ipc_client_for_registration)`. Upstream returns 2-tuple — always use ours.

**`src/agent/agent.rs`** — keep both: our `last_turn_usage` + `push_history()` + upstream fields (`allowed_tools`, `response_cache`, `memory_session_id`, `set_memory_session_id()`).

**`src/agent/loop_.rs`** — keep our `ephemeral_allowlist` guard on MCP + upstream `activated_handle`. Keep `_allowed_tools` (underscore prefix).

**`src/cron/types.rs`** — keep both: our `execution_mode` + `env_overlay` + upstream `allowed_tools`.

**`src/cron/store.rs`** — keep our row indices (17, 18) + upstream `allowed_tools: None`.

**`src/cron/scheduler.rs`** — keep our formatting for Box::pin + add `allowed_tools` param to test initializers.

**`src/gateway/ws.rs`** — keep our `token_prefix` param + session-based arch + writer task `});`. Do NOT take upstream `agent.set_memory_session_id` (we don't have `agent` in scope). Keep operator isolation folding (`op:` prefix).

**`src/security/audit.rs`** — keep BOTH: upstream Merkle chain + our HMAC chain + IPC event types + `AuditEvent::ipc()` constructor. Accept upstream as base, then re-add our IPC events enum variants, `hmac` field, HMAC helpers, HMAC tests.

**`src/daemon/mod.rs`** — accept upstream heartbeat/SIGHUP changes, keep our `broker_registration_loop`.

**`src/main.rs`** — accept upstream Box::pin, add `None` for `allowed_tools` param.

**`src/service/mod.rs`** — merge both: our `--config-dir` + `description` + upstream HOME/DISPLAY env vars.

**`web/src/pages/AgentChat.tsx`** — ALWAYS keep ours (heavy IPC modifications). Upstream i18n can be applied later.

**`web/src/App.tsx`** — keep our IPC routing, accept upstream locale changes but set default to `en`.

## Step 7: Hotspot review

Even without conflicts, review these shared hotspots for semantic changes:

**Config:** `src/config/schema.rs`, `src/config/mod.rs`
**Gateway:** `src/gateway/mod.rs`, `src/gateway/api.rs`, `src/gateway/ws.rs`
**Security:** `src/security/pairing.rs`, `src/security/audit.rs`
**Agent:** `src/agent/agent.rs`, `src/agent/loop_.rs`
**Other:** `src/tools/mod.rs`, `src/onboard/wizard.rs`, `src/cron/scheduler.rs`, `src/daemon/mod.rs`, `src/main.rs`, `src/service/mod.rs`
**Web:** `web/src/App.tsx`, `web/src/components/layout/Sidebar.tsx`, `web/src/lib/ws.ts`, `web/src/types/api.ts`, `web/src/pages/AgentChat.tsx`

## Step 8: Validate

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test -q
```

Fork-invariant tests:
```bash
cargo test gateway::ipc::tests
cargo test tools::agents_ipc::tests
cargo test security::pairing::tests
cargo test security::identity::tests
cargo test security::execution::tests
cargo test gateway::agent_registry::tests
cargo test gateway::chat_db::tests
cargo test broker_proxy
```

## Step 9: Verify fork integrity

Run Explore agent to check 12 critical areas:
1. Tool allowlist invariant (SYNAPSECLAW_ALLOWED_TOOLS)
2. IPC route registration + AppState
3. Token metadata / pairing / authenticate()
4. Agent prompt IPC bootstrap (env vars)
5. IPC tools (7 tools, 3-element return tuple)
6. Execution profiles (fail-closed sandbox)
7. Config schema (AgentsIpcConfig, TokenMetadata)
8. Cron scheduler (execution_mode, env_overlay, allowed_tools)
9. Gateway WS (token_prefix, operator isolation, session-based)
10. Audit dual-chain (Merkle + HMAC, IPC events)
11. process_message signature (session_id)
12. Enterprise tools registration order (before IPC)

## Step 10: Push + PR

```bash
git push -u origin sync/upstream-$SYNC_DATE
```

Create PR via REST API (NOT `gh pr create`):
```bash
gh api repos/panviktor/synapseclaw/pulls -X POST \
  -f title="chore(sync): merge upstream/master into fork ($SYNC_DATE)" \
  -f head="sync/upstream-$SYNC_DATE" \
  -f base="main" \
  -f body="$(cat <<'EOF'
## Summary
...
EOF
)"
```

Return the PR URL.

## Arguments

- No args: full sync workflow
- `check`: only fetch + show divergence, no merge
- `conflicts`: show what would conflict without actually merging (dry run)
