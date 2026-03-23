# Channel Triage: Port vs Defer

Phase 4.0 migrates channels from transport-name branching to capability-driven
`ChannelRegistryPort`. This document classifies which channels we port first
and which we defer as tech debt.

## Decision criteria

- **Production usage** — is it in the running fleet?
- **Architectural benefit** — does porting prove the capability model?
- **Complexity** — LOC, statefulness, feature gates, platform dependencies
- **Demand** — user/market reach

---

## Tier 1 — Port to new architecture (10 channels)

These channels get `ChannelRegistryPort` integration, capability declarations,
and eventually `InboundEnvelope` / `HandleInboundMessage` migration.

| # | Channel | Transport | Why port | Complexity | Notes |
|---|---------|-----------|----------|-----------|-------|
| 1 | **Telegram** | Stateless HTTP | Core fleet, most used, stateless — simplest first port | Low | Draft/streaming, reactions, attachments |
| 2 | **Matrix** | matrix-sdk (stateful) | Core fleet, E2EE — benefits most from cached adapter | Medium | Feature-gated (`channel-matrix`), session persistence |
| 3 | **Slack** | REST + Socket Mode | Core fleet, threads, rich formatting | Low | Socket Mode only for listen(), send() is HTTP |
| 4 | **Discord** | REST + WebSocket | Core fleet, threads, reactions, pins | Medium | WebSocket only for listen(), send() is HTTP |
| 5 | **WhatsApp Cloud** | REST API | High demand, growing market | Low | Cloud API mode only (not Web) |
| 6 | **Signal** | signal-cli REST | Real-time, privacy-focused users | Low | Depends on external signal-cli daemon |
| 7 | **Email** | SMTP + IMAP | Unique transport, enterprise use | Medium | Polling-based receive, SMTP send |
| 8 | **Mattermost** | REST API | Self-hosted Slack alternative, threads | Low | Simple REST, thread support |
| 9 | **Webhook** | Generic HTTP | Infrastructure channel, gateway integration | Low | Generic inbound/outbound, used by gateway |
| 10 | **CLI** | stdin/stdout | Always available, testing/dev, simplest adapter | Low | Zero external deps, good first migration target |

### Port order

1. **CLI** — simplest adapter, proves the migration path works
2. **Telegram** — stateless HTTP, most used in production
3. **Slack** — stateless send, validates threads/reactions capabilities
4. **Discord** — validates WebSocket listen + HTTP send split
5. **Matrix** — validates stateful SDK + E2EE through cached registry
6. **WhatsApp Cloud** — validates REST API pattern at scale
7. **Signal** — validates external daemon dependency
8. **Email** — validates SMTP/IMAP unique transport
9. **Mattermost** — validates self-hosted deployment pattern
10. **Webhook** — validates gateway integration

---

## Tier 2 — Tech debt / defer

These channels remain on the old architecture until Tier 1 migration is complete
and the capability model is proven. They continue to work through the existing
`Channel` trait and `start_channels()` path.

| Channel | LOC | Reason to defer |
|---------|-----|----------------|
| WhatsApp Web | 49K + 47K storage | Browser automation (Chromium), complex session pairing, `whatsapp-web` feature gate |
| Lark/Feishu | 81K | Dual-mode (Lark + Feishu), `channel-lark` feature gate, complex auth |
| DingTalk | 13K | China-specific market, assess demand before investing |
| QQ (Tencent) | 24K | China-specific market, assess demand before investing |
| WeCom | 5K | Webhook-only, minimal benefit from capability model |
| Twitter/X | 17K | API instability, rate limit changes, emerging |
| Reddit | 16K | Polling-based (OAuth2), emerging use case |
| Bluesky | 17K | AT Protocol, emerging platform |
| Notion | 22K | Database polling, not a messaging channel — better as a tool |
| Mochat | 11K | Niche, polling-based, low demand |
| NextcloudTalk | 23K | Niche, self-hosted, low demand |
| Linq | 37K | Partner API, specialized B2B |
| WATI | 16K | WhatsApp Business proxy, use WhatsApp Cloud instead |
| Nostr | 15K | `channel-nostr` feature gate, experimental protocol |
| iMessage | 49K | macOS-only, platform-specific, AppleScript dependency |
| IRC | 36K | Legacy protocol, low demand, complex (NickServ, SASL, TLS) |
| ClawdTalk | 13K | Internal/proprietary voice channel |

### Review policy

- Re-assess Tier 2 channels **quarterly** or when user demand changes
- Channels that gain production users can be promoted to Tier 1
- Channels that lose upstream maintenance can be deprecated

---

## Capability map (Tier 1 channels)

| Channel | SendText | ReceiveText | Threads | Reactions | Typing | Attachments | RichFormatting | EditMessage |
|---------|----------|-------------|---------|-----------|--------|-------------|---------------|------------|
| Telegram | Y | Y | Y | Y | Y | Y | Y (HTML) | Y |
| Matrix | Y | Y | Y | Y | Y | Y | Y (HTML) | - |
| Slack | Y | Y | Y | Y | Y | Y | Y (mrkdwn) | - |
| Discord | Y | Y | Y | Y | Y | Y | Y (Markdown) | Y |
| WhatsApp | Y | Y | - | Y | - | Y | - | - |
| Signal | Y | Y | - | Y | - | Y | - | - |
| Email | Y | Y | Y (Re:) | - | - | Y | Y (HTML) | - |
| Mattermost | Y | Y | Y | Y | Y | Y | Y (Markdown) | - |
| Webhook | Y | Y | - | - | - | - | - | - |
| CLI | Y | Y | - | - | - | - | - | - |

---

## Related

- [`ipc-phase4_0-plan.md`](ipc-phase4_0-plan.md) — full Phase 4.0 architecture plan
- [`ipc-phase4_0-progress.md`](ipc-phase4_0-progress.md) — execution progress
- [`delta-registry.md`](delta-registry.md) — fork delta inventory (CORE-001)
