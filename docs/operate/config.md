# Config

Main config normally lives under `~/.synapseclaw/config.toml`. Helper agent configs live under `~/.synapseclaw/agents/*/config.toml`.

Operational secrets should live outside tracked repository files. On Linux installs that run through the user systemd service, prefer `~/.config/systemd/user/synapseclaw.env` for provider keys such as `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `OPENROUTER_API_KEY`, `GROQ_API_KEY`, `MISTRAL_API_KEY`, `MINIMAX_API_KEY`, and `XAI_API_KEY`; reference them from config with `api_key_env`.

For skills, important settings include auto-promotion policy, open-skills sources, and workspace package porting.

Compaction, embeddings, cheap reasoning, speech transcription, speech synthesis, and media helpers are configured through `[[model_lanes]]`; see [../reference/model-lanes.md](../reference/model-lanes.md). Legacy summary and embedding routing keys are rejected at config load.
