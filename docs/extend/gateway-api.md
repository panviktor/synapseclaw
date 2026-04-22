# Gateway API

The gateway API is the shared backend used by the web UI and by CLI fallback flows when the daemon is running. It should expose runtime behavior without creating a second implementation path.

Skills have the most complete gateway API today. See [../reference/skills-api.md](../reference/skills-api.md) for exact endpoints and [add-skill-support.md](add-skill-support.md) for implementation rules.

Realtime call endpoints follow the same rule: gateway handlers should call the typed call runtime port and channel capability profile, or the shared session-ledger helper for read-only inspection, not hardcode transport behavior in web-only code. See [../reference/realtime-calls.md](../reference/realtime-calls.md).
