# Config

Main config normally lives under `~/.synapseclaw/config.toml`. Helper agent configs live under `~/.synapseclaw/agents/*/config.toml`.

Operational secrets should live outside tracked repository files. On Linux installs that run through the user systemd service, prefer `~/.config/systemd/user/synapseclaw.env` for provider keys such as `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `OPENROUTER_API_KEY`, `GROQ_API_KEY`, `MISTRAL_API_KEY`, `MINIMAX_API_KEY`, and `XAI_API_KEY`; reference them from config with `api_key_env`.

For the simple onboarding flow:

- Linux stores onboarding-managed secrets in `~/.config/systemd/user/synapseclaw.env`
- macOS stores onboarding-managed secrets in `~/.synapseclaw/synapseclaw.env`

The current simple wizard writes provider secrets there and also uses that env-file path for first-channel secrets such as `TELEGRAM_BOT_TOKEN` or `MATRIX_ACCESS_TOKEN`.

If you install the user service after onboarding, the service loader uses the same env file. That keeps the first-run path and the always-on runtime on the same secret contract.

For skills, important settings include auto-promotion policy, open-skills sources, and workspace package porting.

Compaction, embeddings, cheap reasoning, speech transcription, speech synthesis, and media helpers are configured through `[[model_lanes]]`; see [../reference/model-lanes.md](../reference/model-lanes.md). Legacy summary and embedding routing keys are rejected at config load.
