---
name: fleet-status
description: "Quick health check of the SynapseClaw fleet — all 6 systemd services, binary version, recent errors. Use when the user says 'fleet status', 'статус', 'как агенты', 'все ок?', 'check fleet', 'are agents running', 'что с ботами', 'health check', or wants a quick overview of the running system."
user-invocable: true
---

# Fleet Status

Quick health overview of all running SynapseClaw services.

## Step 1: Service status

Run in parallel:

```bash
systemctl --user is-active synapseclaw.service synapseclaw@{copywriter,marketing-lead,news-reader,publisher,trend-aggregator}.service
```

```bash
synapseclaw --version
```

## Step 2: Uptime and memory

```bash
systemctl --user status synapseclaw.service synapseclaw@{copywriter,marketing-lead,news-reader,publisher,trend-aggregator}.service --no-pager 2>&1 | grep -E '(Active:|Memory:|Main PID:)'
```

## Step 3: Recent errors

Check for errors in the last 10 minutes across all services:

```bash
journalctl --user -u 'synapseclaw*' --since "10 min ago" -p err --no-pager -q 2>/dev/null | tail -20
```

## Step 4: Report

Present a clean summary table:

```
| Service            | Status | Uptime     | Errors (10m) |
|--------------------|--------|------------|--------------|
| daemon             | ✅     | 2h 15m    | 0            |
| copywriter         | ✅     | 2h 15m    | 0            |
| marketing-lead     | ✅     | 2h 15m    | 0            |
| news-reader        | ✅     | 2h 15m    | 0            |
| publisher          | ✅     | 2h 15m    | 0            |
| trend-aggregator   | ✅     | 2h 15m    | 0            |
```

If any service is down or has errors, highlight it and show the relevant log excerpt.

## Arguments

- No args: full fleet report
- `<agent-name>`: detailed status + recent logs for one specific agent
