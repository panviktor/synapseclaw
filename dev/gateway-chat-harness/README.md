# Gateway Chat Harness

Local dev harness for driving a live SynapseClaw agent over the gateway WebSocket.

By default the harness primes the session onto the cheap route:

```text
/model cheap
```

Use `--route gpt-5.4` for OpenAI-specific continuation/history experiments.

Examples:

```bash
cargo run --manifest-path dev/gateway-chat-harness/Cargo.toml -- \
  -m "Reply with exactly HELLO."
```

```bash
cargo run --manifest-path dev/gateway-chat-harness/Cargo.toml -- \
  --session mem-smoke \
  --history \
  -m "Запомни на эту рабочую цепочку: проект Atlas, ветка release/hotfix-17, staging URL https://staging.atlas.local, главный риск — логин через SSO." \
  -m "Не про общий проект, а именно про текущую рабочую цепочку: какой проект, какая ветка, какой staging URL и какой главный риск?"
```

Helper agent via broker proxy:

```bash
cargo run --manifest-path dev/gateway-chat-harness/Cargo.toml -- \
  --agent copywriter \
  -m "Reply with exactly HELLO."
```

Force an OpenAI-family route for a specific run:

```bash
cargo run --manifest-path dev/gateway-chat-harness/Cargo.toml -- \
  --route gpt-5.4 \
  -m "Reply with exactly HELLO."
```

## Phase 4.10 Live Pack

Run the phase-close provider/capability/context pack:

```bash
dev/gateway-chat-harness/scripts/phase4_10_live_pack.sh
```

Default coverage:

- provider smokes for `cheap`, `deepseek`, and `gpt-5.4`
- tool-call, memory recall, and CJK recall checks
- structured media admission markers for image/audio/video/music lanes
- systemd journal capture for provider-context budget, admission, embedding, and compaction signals

Optional expensive checks:

```bash
RUN_HEAVY=1 dev/gateway-chat-harness/scripts/phase4_10_live_pack.sh
```

Use `RUN_HEAVY=1` only at slice-close points; it drives a long semantic dialogue to verify compaction, context size, semantic retention, and that pure dialogue does not create procedural skills.

Budget enforcement knobs:

- `STRICT_CONTEXT_BUDGET=1` turns provider-context over-budget warnings into failures.
- `CONTEXT_WARN_MAX_CHARS=7000` controls the default max-char warning ceiling.
- `REQUIRE_EMBEDDING_SIGNAL=1` turns missing embedding logs into a failure.
- `RUN_DOCTOR_MODELS=1` runs provider catalog probes; by default it imports `~/.config/systemd/user/synapseclaw.env` for API-key-only providers without printing secrets.
