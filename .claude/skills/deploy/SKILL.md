---
name: deploy
description: "Build SynapseClaw from source and deploy to running fleet. Runs cargo build with --features channel-matrix, stops all 6 systemd services, copies binary, restarts services. Use when the user says 'deploy', 'деплой', 'обнови бинарник', 'build and restart', 'пересобери', 'задеплой', 'rebuild', or after merging a PR to main."
user-invocable: true
---

# Deploy SynapseClaw

Build from source and hot-swap the running binary across the fleet.

## Prerequisites

Must be on `main` branch with clean working tree. If not, warn the user.

## Step 1: Verify state

```bash
git branch --show-current
git status --short
```

If not on `main` or tree is dirty, stop and tell the user.

## Step 2: Build

Matrix channel support is required — always build with this feature:

```bash
cargo build --release --features channel-matrix
```

If build fails, report errors and stop. Do not proceed to service restart with a failed build.

## Step 3: Stop all services

```bash
systemctl --user stop synapseclaw.service synapseclaw@{copywriter,marketing-lead,news-reader,publisher,trend-aggregator}.service
```

## Step 4: Replace binary

```bash
cp target/release/synapseclaw ~/.cargo/bin/synapseclaw
```

Verify:
```bash
synapseclaw --version
```

## Step 5: Start all services

```bash
systemctl --user start synapseclaw.service synapseclaw@{copywriter,marketing-lead,news-reader,publisher,trend-aggregator}.service
```

## Step 6: Health check

Wait 3 seconds, then verify all services are running:

```bash
sleep 3
systemctl --user is-active synapseclaw.service synapseclaw@{copywriter,marketing-lead,news-reader,publisher,trend-aggregator}.service
```

If any service failed to start, show its logs:
```bash
journalctl --user -u <failed-service> --since "30 sec ago" --no-pager
```

Report final status to the user.

## Arguments

- No args: full deploy (build → stop → copy → start → health check)
- `build`: only build, don't restart services
- `restart`: skip build, just restart services (useful after manual binary copy)
