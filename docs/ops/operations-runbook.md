# SynapseClaw Operations Runbook

This runbook is for operators who maintain availability, security posture, and incident response.

Last verified: **February 18, 2026**.

## Scope

Use this document for day-2 operations:

- starting and supervising runtime
- health checks and diagnostics
- safe rollout and rollback
- incident triage and recovery

For first-time installation, start from [one-click-bootstrap.md](../setup-guides/one-click-bootstrap.md).

## Runtime Modes

| Mode | Command | When to use |
|---|---|---|
| Foreground runtime | `synapseclaw daemon` | local debugging, short-lived sessions |
| Foreground gateway only | `synapseclaw gateway` | webhook endpoint testing |
| User service | `synapseclaw service install && synapseclaw service start` | persistent operator-managed runtime |
| Docker / Podman | `docker compose up -d` | containerized deployment |

## Docker / Podman Runtime

If you installed via `./install.sh --docker`, the container exits after onboarding. To run
SynapseClaw as a long-lived container, use the repository `docker-compose.yml` or start a
container manually against the persisted data directory.

### Recommended: docker-compose

```bash
# Start (detached, auto-restarts on reboot)
docker compose up -d

# Stop
docker compose down

# Restart
docker compose up -d
```

Replace `docker` with `podman` if using Podman.

### Manual container lifecycle

```bash
# Start a new container from the bootstrap image
docker run -d --name synapseclaw \
  --restart unless-stopped \
  -v "$PWD/.synapseclaw-docker/.synapseclaw:/synapseclaw-data/.synapseclaw" \
  -v "$PWD/.synapseclaw-docker/workspace:/synapseclaw-data/workspace" \
  -e HOME=/synapseclaw-data \
  -e SYNAPSECLAW_WORKSPACE=/synapseclaw-data/workspace \
  -p 42617:42617 \
  synapseclaw-bootstrap:local \
  gateway

# Stop (preserves config and workspace)
docker stop synapseclaw

# Restart a stopped container
docker start synapseclaw

# View logs
docker logs -f synapseclaw

# Health check
docker exec synapseclaw synapseclaw status
```

For Podman, add `--userns keep-id --user "$(id -u):$(id -g)"` and append `:Z` to volume mounts.

### Key detail: do not re-run install.sh to restart

Re-running `install.sh --docker` rebuilds the image and re-runs onboarding. To simply
restart, use `docker start`, `docker compose up -d`, or `podman start`.

For full setup instructions, see [one-click-bootstrap.md](../setup-guides/one-click-bootstrap.md#stopping-and-restarting-a-dockerpodman-container).

## Baseline Operator Checklist

1. Validate configuration:

```bash
synapseclaw status
```

2. Verify diagnostics:

```bash
synapseclaw doctor
synapseclaw channel doctor
```

3. Start runtime:

```bash
synapseclaw daemon
```

4. For persistent user session service:

```bash
synapseclaw service install
synapseclaw service start
synapseclaw service status
```

## Health and State Signals

| Signal | Command / File | Expected |
|---|---|---|
| Config validity | `synapseclaw doctor` | no critical errors |
| Channel connectivity | `synapseclaw channel doctor` | configured channels healthy |
| Runtime summary | `synapseclaw status` | expected provider/model/channels |
| Daemon heartbeat/state | `~/.synapseclaw/daemon_state.json` | file updates periodically |

## Logs and Diagnostics

### macOS / Windows (service wrapper logs)

- `~/.synapseclaw/logs/daemon.stdout.log`
- `~/.synapseclaw/logs/daemon.stderr.log`

### Linux (systemd user service)

```bash
journalctl --user -u synapseclaw.service -f
```

## Incident Triage Flow (Fast Path)

1. Snapshot system state:

```bash
synapseclaw status
synapseclaw doctor
synapseclaw channel doctor
```

2. Check service state:

```bash
synapseclaw service status
```

3. If service is unhealthy, restart cleanly:

```bash
synapseclaw service stop
synapseclaw service start
```

4. If channels still fail, verify allowlists and credentials in `~/.synapseclaw/config.toml`.

5. If gateway is involved, verify bind/auth settings (`[gateway]`) and local reachability.

## Safe Change Procedure

Before applying config changes:

1. backup `~/.synapseclaw/config.toml`
2. apply one logical change at a time
3. run `synapseclaw doctor`
4. restart daemon/service
5. verify with `status` + `channel doctor`

## Rollback Procedure

If a rollout regresses behavior:

1. restore previous `config.toml`
2. restart runtime (`daemon` or `service`)
3. confirm recovery via `doctor` and channel health checks
4. document incident root cause and mitigation

## Related Docs

- [one-click-bootstrap.md](../setup-guides/one-click-bootstrap.md)
- [troubleshooting.md](./troubleshooting.md)
- [config-reference.md](../reference/api/config-reference.md)
- [commands-reference.md](../reference/cli/commands-reference.md)
