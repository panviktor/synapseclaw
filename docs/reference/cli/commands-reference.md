# SynapseClaw Commands Reference

This reference is derived from the current CLI surface (`synapseclaw --help`).

Last verified: **February 21, 2026**.

## Top-Level Commands

| Command | Purpose |
|---|---|
| `onboard` | Initialize workspace/config quickly or interactively |
| `agent` | Run interactive chat or single-message mode |
| `gateway` | Start webhook and WhatsApp HTTP gateway |
| `daemon` | Start supervised runtime (gateway + channels + optional heartbeat/scheduler) |
| `service` | Manage user-level OS service lifecycle |
| `doctor` | Run diagnostics and freshness checks |
| `status` | Print current configuration and system summary |
| `estop` | Engage/resume emergency stop levels and inspect estop state |
| `cron` | Manage scheduled tasks |
| `models` | Refresh provider model catalogs |
| `providers` | List provider IDs, aliases, and active provider |
| `channel` | Manage channels and channel health checks |
| `integrations` | Inspect integration details |
| `skills` | List/install/remove skills |
| `migrate` | Import from external runtimes (currently OpenClaw) |
| `config` | Export machine-readable config schema |
| `completions` | Generate shell completion scripts to stdout |
| `hardware` | Discover and introspect USB hardware |
| `peripheral` | Configure and flash peripherals |

## Command Groups

### `onboard`

- `synapseclaw onboard`
- `synapseclaw onboard --channels-only`
- `synapseclaw onboard --force`
- `synapseclaw onboard --reinit`
- `synapseclaw onboard --api-key <KEY> --provider <ID> --memory <sqlite|lucid|markdown|none>`
- `synapseclaw onboard --api-key <KEY> --provider <ID> --model <MODEL_ID> --memory <sqlite|lucid|markdown|none>`
- `synapseclaw onboard --api-key <KEY> --provider <ID> --model <MODEL_ID> --memory <sqlite|lucid|markdown|none> --force`

`onboard` safety behavior:

- If `config.toml` already exists, onboarding offers two modes:
  - Full onboarding (overwrite `config.toml`)
  - Provider-only update (update provider/model/API key while preserving existing channels, tunnel, memory, hooks, and other settings)
- Guided onboarding is preset-first:
  - `ChatGPT / Codex`
  - `Claude`
  - `OpenRouter`
  - `Local`
  - `Advanced`
- Presets expand into lane-aware routing (`reasoning`, `cheap_reasoning`, `embedding`, and later multimodal lanes) while still allowing manual overrides in config.
- For deeper local customization of preset seeds, curated model lists, and provider defaults, use `synapseclaw models catalog init` and edit the generated `model_catalog.json` next to `config.toml`.
- In non-interactive environments, existing `config.toml` causes a safe refusal unless `--force` is passed.
- Use `synapseclaw onboard --channels-only` when you only need to rotate channel tokens/allowlists.
- Use `synapseclaw onboard --reinit` to start fresh. This backs up your existing config directory with a timestamp suffix and creates a new configuration from scratch.

### `agent`

- `synapseclaw agent`
- `synapseclaw agent -m "Hello"`
- `synapseclaw agent --provider <ID> --model <MODEL> --temperature <0.0-2.0>`
- `synapseclaw agent --peripheral <board:path>`

Tip:

- In interactive chat, you can ask for route changes in natural language (for example “conversation uses kimi, coding uses gpt-5.3-codex”); the assistant can persist this via tool `model_routing_config`.

### `gateway` / `daemon`

- `synapseclaw gateway [--host <HOST>] [--port <PORT>]`
- `synapseclaw daemon [--host <HOST>] [--port <PORT>]`

### `estop`

- `synapseclaw estop` (engage `kill-all`)
- `synapseclaw estop --level network-kill`
- `synapseclaw estop --level domain-block --domain "*.chase.com" [--domain "*.paypal.com"]`
- `synapseclaw estop --level tool-freeze --tool shell [--tool browser]`
- `synapseclaw estop status`
- `synapseclaw estop resume`
- `synapseclaw estop resume --network`
- `synapseclaw estop resume --domain "*.chase.com"`
- `synapseclaw estop resume --tool shell`
- `synapseclaw estop resume --otp <123456>`

Notes:

- `estop` commands require `[security.estop].enabled = true`.
- When `[security.estop].require_otp_to_resume = true`, `resume` requires OTP validation.
- OTP prompt appears automatically if `--otp` is omitted.

### `service`

- `synapseclaw service install`
- `synapseclaw service start`
- `synapseclaw service stop`
- `synapseclaw service restart`
- `synapseclaw service status`
- `synapseclaw service uninstall`

### `cron`

- `synapseclaw cron list`
- `synapseclaw cron add <expr> [--tz <IANA_TZ>] <command>`
- `synapseclaw cron add-at <rfc3339_timestamp> <command>`
- `synapseclaw cron add-every <every_ms> <command>`
- `synapseclaw cron once <delay> <command>`
- `synapseclaw cron remove <id>`
- `synapseclaw cron pause <id>`
- `synapseclaw cron resume <id>`

Notes:

- Mutating schedule/cron actions require `cron.enabled = true`.
- Shell command payloads for schedule creation (`create` / `add` / `once`) are validated by security command policy before job persistence.

### `models`

- `synapseclaw models refresh`
- `synapseclaw models refresh --provider <ID>`
- `synapseclaw models refresh --force`
- `synapseclaw models list [--provider <ID>]`
- `synapseclaw models set <MODEL_ID>`
- `synapseclaw models status`
- `synapseclaw models catalog init [--force]`
- `synapseclaw models catalog status`
- `synapseclaw models catalog path`

`models refresh` currently supports live catalog refresh for provider IDs: `openrouter`, `openai`, `anthropic`, `groq`, `mistral`, `deepseek`, `xai`, `together-ai`, `gemini`, `ollama`, `llamacpp`, `sglang`, `vllm`, `astrai`, `venice`, `fireworks`, `cohere`, `moonshot`, `glm`, `zai`, `qwen`, and `nvidia`.

`models catalog init` writes a local editable `model_catalog.json` next to the
active `config.toml`. On startup SynapseClaw merges that file over the built-in
catalog. Use this to override built-in presets, provider defaults, curated
model lists, or default pricing without changing repository files.

### `doctor`

- `synapseclaw doctor`
- `synapseclaw doctor models [--provider <ID>] [--use-cache]`
- `synapseclaw doctor traces [--limit <N>] [--event <TYPE>] [--contains <TEXT>]`
- `synapseclaw doctor traces --id <TRACE_ID>`

`doctor traces` reads runtime tool/model diagnostics from `observability.runtime_trace_path`.

### `channel`

- `synapseclaw channel list`
- `synapseclaw channel start`
- `synapseclaw channel doctor`
- `synapseclaw channel bind-telegram <IDENTITY>`
- `synapseclaw channel add <type> <json>`
- `synapseclaw channel remove <name>`

Runtime in-chat commands (Telegram/Discord while channel server is running):

- `/models`
- `/models <provider>`
- `/model`
- `/model <model-id>`
- `/new`

Channel runtime also watches `config.toml` and hot-applies updates to:
- `default_provider`
- `default_model`
- `default_temperature`
- `api_key` / `api_url` (for the default provider)
- `reliability.*` provider retry settings

`add/remove` currently route you back to managed setup/manual config paths (not full declarative mutators yet).

### `integrations`

- `synapseclaw integrations info <name>`

### `skills`

- `synapseclaw skills list`
- `synapseclaw skills audit <source_or_name>`
- `synapseclaw skills install <source>`
- `synapseclaw skills remove <name>`

`<source>` accepts git remotes (`https://...`, `http://...`, `ssh://...`, and `git@host:owner/repo.git`) or a local filesystem path.

`skills install` always runs a built-in static security audit before the skill is accepted. The audit blocks:
- symlinks inside the skill package
- script-like files (`.sh`, `.bash`, `.zsh`, `.ps1`, `.bat`, `.cmd`)
- high-risk command snippets (for example pipe-to-shell payloads)
- markdown links that escape the skill root, point to remote markdown, or target script files

Use `skills audit` to manually validate a candidate skill directory (or an installed skill by name) before sharing it.

Skill manifests (`SKILL.toml`) support `prompts` and `[[tools]]`; both are injected into the agent system prompt at runtime, so the model can follow skill instructions without manually reading skill files.

### `migrate`

- `synapseclaw migrate openclaw [--source <path>] [--dry-run]`

### `config`

- `synapseclaw config schema`

`config schema` prints a JSON Schema (draft 2020-12) for the full `config.toml` contract to stdout.

### `completions`

- `synapseclaw completions bash`
- `synapseclaw completions fish`
- `synapseclaw completions zsh`
- `synapseclaw completions powershell`
- `synapseclaw completions elvish`

`completions` is stdout-only by design so scripts can be sourced directly without log/warning contamination.

### `hardware`

- `synapseclaw hardware discover`
- `synapseclaw hardware introspect <path>`
- `synapseclaw hardware info [--chip <chip_name>]`

### `peripheral`

- `synapseclaw peripheral list`
- `synapseclaw peripheral add <board> <path>`
- `synapseclaw peripheral flash [--port <serial_port>]`
- `synapseclaw peripheral setup-uno-q [--host <ip_or_host>]`
- `synapseclaw peripheral flash-nucleo`

## Validation Tip

To verify docs against your current binary quickly:

```bash
synapseclaw --help
synapseclaw <command> --help
```
