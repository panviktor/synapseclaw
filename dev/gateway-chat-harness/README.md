# Gateway Chat Harness

Local dev harness for driving a live SynapseClaw agent over the gateway WebSocket.

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
