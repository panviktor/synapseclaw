---
name: pre-pr
description: "Run full pre-PR validation (fmt, clippy, tests), then commit, push, and create a PR via GitHub REST API. Use when the user says 'делай пр', 'создай пр', 'pre-pr', 'push and PR', 'submit for review', or wants to validate and ship current changes."
user-invocable: true
---

# Pre-PR: Validate → Commit → Push → PR

Complete workflow from local changes to merged-ready PR.

## Step 1: Validate

Run in parallel:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
```

If either fails, fix the issues first. Re-run until clean.

Then run tests:
```bash
cargo test -q
```

Collect total passed count from all `test result:` lines.

If any step fails, stop and report. Do not create a PR with failing checks.

## Step 2: Commit

If there are uncommitted changes:

1. `git status` — see what changed
2. `git diff` — review changes
3. `git log --oneline -5` — match commit message style

Stage specific files (not `git add -A`). Create commit with conventional title:

```bash
git commit -m "$(cat <<'EOF'
<type>(<scope>): <description>

<body if needed>

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
EOF
)"
```

If changes are already committed, skip this step.

## Step 3: Branch + Push

If on `main`, create a feature branch first:
```bash
git checkout -b <type>/<descriptive-name>
```

Branch naming: `feat/`, `fix/`, `docs/`, `chore/`, `refactor/`

Push:
```bash
git push -u origin <branch>
```

## Step 4: Create PR

Use GitHub REST API (NOT `gh pr create` — it fails with token scope error on this repo):

```bash
gh api repos/panviktor/zeroclaw/pulls -X POST \
  -f title="<conventional title>" \
  -f head="<branch>" \
  -f base="main" \
  -f body="$(cat <<'EOF'
## Summary
<1-3 bullet points describing what changed and why>

## Changes
<list of files/areas changed>

## Test plan
- [x] `cargo fmt --all -- --check` — clean
- [x] `cargo clippy --all-targets -- -D warnings` — clean
- [x] `cargo test -q` — <N> passed

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

Return the PR URL.

## Arguments

- No args: full workflow (validate → commit → push → PR)
- `validate`: only run fmt + clippy + tests, no commit/push/PR
- `commit`: validate + commit, no push/PR
- `push`: validate + commit + push, no PR

Example: `/pre-pr validate` — just check if code is clean.
