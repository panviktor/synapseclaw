---
name: synapseclaw
description: "Help users operate their SynapseClaw agent instance — CLI commands, gateway API, memory, cron, channels, providers, config, troubleshooting. Trigger when the user says anything like: 'check status', 'send message', 'add cron job', 'list memory', 'set up telegram', 'configure provider', 'my bot is broken', 'check logs', 'restart agent', 'run diagnostics', 'pair a new agent', 'quarantine agent', or any operation involving the synapseclaw binary or its HTTP endpoints."
user-invocable: true
---

# SynapseClaw Operations

Operate SynapseClaw via CLI and gateway API.

## Fleet

6 systemd user services share one env file (`~/.config/systemd/user/synapseclaw.env`):

| Service | Config |
|---------|--------|
| `synapseclaw.service` | `~/.synapseclaw/` |
| `synapseclaw@copywriter.service` | `~/.synapseclaw/agents/copywriter/` |
| `synapseclaw@marketing-lead.service` | `~/.synapseclaw/agents/marketing-lead/` |
| `synapseclaw@news-reader.service` | `~/.synapseclaw/agents/news-reader/` |
| `synapseclaw@publisher.service` | `~/.synapseclaw/agents/publisher/` |
| `synapseclaw@trend-aggregator.service` | `~/.synapseclaw/agents/trend-aggregator/` |

Binary: `~/.cargo/bin/synapseclaw`

## CLI

```
synapseclaw status          # System status
synapseclaw doctor          # Diagnostics
synapseclaw daemon          # Full runtime
synapseclaw agent           # Interactive agent loop
synapseclaw gateway         # Gateway server only
synapseclaw service         # Manage systemd service
synapseclaw estop           # Emergency stop
synapseclaw cron            # Scheduled tasks
synapseclaw memory          # Memory (list, get, stats, clear)
synapseclaw channel         # Manage channels
synapseclaw config          # Configuration
synapseclaw models          # Model catalogs
synapseclaw providers       # List providers
synapseclaw integrations    # Browse integrations
synapseclaw auth            # Provider auth profiles
synapseclaw audit           # Verify HMAC audit chain
synapseclaw hardware        # USB discovery
synapseclaw peripheral      # STM32, RPi GPIO
```

Per-agent: `synapseclaw --config-dir ~/.synapseclaw/agents/<name> <command>`

## Gateway API

Default `http://127.0.0.1:42617`, bearer token required.

```
GET  /api/status              POST /api/message
GET  /api/memory              POST /api/memory
GET  /api/cron                POST /api/cron
DELETE /api/cron/:id

# IPC
GET  /api/ipc/agents          POST /api/ipc/send
GET  /api/ipc/inbox           POST /api/ipc/state
GET  /api/ipc/state?key=...

# Admin (localhost)
POST /admin/paircode/new      POST /admin/ipc/quarantine
POST /admin/ipc/revoke        POST /admin/ipc/disable
POST /admin/ipc/downgrade
```

## Service commands

```bash
# Logs
journalctl --user -u synapseclaw.service -f
journalctl --user -u synapseclaw@copywriter.service --since "10 min ago"

# Restart one agent
systemctl --user restart synapseclaw@copywriter.service
```

## Memory (Phase 4.3 — SurrealDB)

```bash
synapseclaw memory list [--category core] [--limit 20]
synapseclaw memory get <key>
synapseclaw memory stats
synapseclaw memory clear --key <key> [--yes]
synapseclaw memory migrate [--yes]     # SQLite → SurrealDB one-time migration
```

Per-agent: `synapseclaw --config-dir ~/.synapseclaw/agents/<name> memory <command>`

Backend: SurrealDB 3.0 embedded (`brain.surreal`).
Embeddings: OpenRouter Qwen3 Embedding 8B (4096 dims).
Config: `[memory]` section in config.toml.

Tools available to agents:
- `memory_store` / `memory_recall` / `memory_forget`
- `core_memory_update` — MemGPT core blocks (persona, user_knowledge, task_state, domain)
- `knowledge` — entity graph (search, add_entity, add_fact, get_facts)

## Troubleshooting

1. **Not responding** → `synapseclaw doctor` + check journalctl
2. **Gateway down** → verify port 42617, `synapseclaw status`
3. **Token issues** → check `~/.config/systemd/user/synapseclaw.env`, then `systemctl --user daemon-reload`
4. **IPC issues** → check `[agents_ipc]` in config.toml
