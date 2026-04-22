# Getting Started Docs

For first-time setup and quick orientation.

## Start Path

1. Main overview and quick start: [../../README.md](../../README.md)
2. One-click setup and dual bootstrap mode: [one-click-bootstrap.md](one-click-bootstrap.md)
3. Update or uninstall on macOS: [macos-update-uninstall.md](macos-update-uninstall.md)
4. Find commands by tasks: [../reference/cli/commands-reference.md](../reference/cli/commands-reference.md)

## Choose Your Path

| Scenario | Command |
|----------|---------|
| I have an API key, want fastest setup | `synapseclaw onboard --api-key sk-... --provider openrouter` |
| I want guided preset-first setup | `synapseclaw onboard` |
| I want to tweak built-in presets/default models locally | `synapseclaw models catalog init` |
| Config exists, just fix channels | `synapseclaw onboard --channels-only` |
| Config exists, I intentionally want full overwrite | `synapseclaw onboard --force` |
| Using subscription auth | See [Subscription Auth](../../README.md#subscription-auth-openai-codex--claude-code) |

## Onboarding and Validation

- Quick onboarding: `synapseclaw onboard --api-key "sk-..." --provider openrouter`
- Guided onboarding: `synapseclaw onboard`
  - starts with simple presets (`ChatGPT / Codex`, `Claude`, `OpenRouter`, `Local`, `Advanced`)
  - expands them into lane-aware routing under the hood
- Existing config protection: reruns require explicit confirmation (or `--force` in non-interactive flows)
- Ollama cloud models (`:cloud`) require a remote `api_url` and API key (for example `api_url = "https://ollama.com"`).
- Validate environment: `synapseclaw status` + `synapseclaw doctor`

## Model Catalog

SynapseClaw ships with a **built-in model catalog** (`model_catalog.json`) embedded in the binary.
It contains presets, 30+ providers, curated model lists, pricing, context-window profiles,
embedding profiles, and route aliases (shortcuts like `cheap`, `qwen36`, `gemma31b`).

To override any part of it locally:

```bash
synapseclaw models catalog init
```

This writes `model_catalog.json` next to your `config.toml`:

| Location | Scope |
|----------|-------|
| `~/.synapseclaw/model_catalog.json` | Global (all agents) |
| `~/.synapseclaw/agents/<name>/model_catalog.json` | Per-agent |

### How the override works

- **Merge, not replace** — the local file is merged over the built-in catalog on startup
- Matching entries (by `id`, `provider`, `model`, or `hint`) **replace** built-in values
- New entries are **added** — you can introduce custom providers, presets, or aliases
- Missing sections are ignored — you only need to include the parts you want to change

### Useful commands

| Command | What it does |
|---------|-------------|
| `synapseclaw models catalog init` | Create editable catalog from built-in seed |
| `synapseclaw models catalog init --force` | Overwrite existing catalog |
| `synapseclaw models catalog status` | Show path and active/not active |
| `synapseclaw models catalog path` | Print catalog file path |
| `synapseclaw models refresh` | Pull latest models from provider APIs |
| `synapseclaw models list` | List cached provider models |
| `synapseclaw models set <model>` | Set default model in config |
| `synapseclaw models status` | Show current model configuration |

### Catalog structure

```json
{
  "presets": [{ "id": "...", "title": "...", "default_provider": "...", "default_model": "...", "extra_lanes": [...] }],
  "providers": [{ "provider": "...", "default_model": "...", "api_base_urls": [...], "curated_models": [...] }],
  "pricing": [{ "model": "...", "input": 1.0, "output": 5.0 }],
  "profiles": [{ "provider": "...", "model": "...", "context_window_tokens": 128000, "max_output_tokens": 65536, "features": ["tool_calling"] }],
  "embedding_profiles": [{ "provider": "...", "model": "...", "dimensions": 1024, "distance_metric": "cosine", ... }],
  "route_aliases": [{ "hint": "myalias", "provider": "...", "model": "..." }]
}
```

## Next

- Runtime operations: [../ops/README.md](../ops/README.md)
- Reference catalogs: [../reference/README.md](../reference/README.md)
- macOS lifecycle tasks: [macos-update-uninstall.md](macos-update-uninstall.md)
