# Config Reference

Main config lives under `~/.synapseclaw/config.toml`. Helper agents use `~/.synapseclaw/agents/*/config.toml`, and secrets should be supplied through the local service environment.

Important Skills-related config areas include auto-promotion policy, open-skills sources, and workspace package porting. Auxiliary model selection uses [model lanes](model-lanes.md); do not use legacy summary or embedding routing keys.

For live voice calls, also look at:

- `[transcription]` and provider-specific realtime speech settings such as `[transcription.deepgram.flux]`
- `[tts]` for spoken replies
- `[agent.live_calls]` for live-call model, reply budget, excluded tools, locale fallback, and greetings

For memory-related behavior:

- `embedding` model lanes control vector recall and compact semantic discovery
- implicit memory recall uses normal memory configuration and diagnostics; it does not have a separate legacy keyword-router section
- runtime usage, pressure, and watchdog reporting are diagnostic outputs built from typed runtime state, not a prompt-configurable prose layer

See [realtime-calls.md](realtime-calls.md) for a complete working Matrix live-call example.
