# SynapseClaw News & Changelog

## 2026-03-23

### Phase 4.0 Step 4 + cleanup: RunStorePort + send_channel_message removal
- `Run`, `RunEvent`, `RunState`, `RunOrigin`, `RunEventType` domain types
- `RunStorePort` trait — unified CRUD for execution runs and events
- `ChatDbRunStore` adapter with `runs` + `run_events` SQLite tables (migration added to ChatDb)
- `ChatDb::conn()` public accessor for adapter reuse
- Cleanup: `send_channel_message()` deleted, inlined at CLI `channel send` handler

### Phase 4.0 Step 3: ConversationStorePort + conversation domain types
- `ConversationSession`, `ConversationEvent`, `EventType`, `ConversationKind` domain types
- `ConversationStorePort` trait — unified CRUD for sessions, transcript events, summaries
- `ChatDbConversationStore` adapter wrapping existing `ChatDb` SQLite backend
- Fix: `InboundEnvelope::conversation_ref` for threaded messages now includes channel prefix (was missing, caused history key mismatch)

### Phase 4.0 Step 7: InboundEnvelope + HandleInboundMessage
- `InboundEnvelope` domain type — canonical input for all inbound messages (channel, web, IPC, cron)
- `SourceKind` enum (Channel, Web, Ipc, Cron, System)
- `InboundEnvelope::from_channel_message()` — adapter→core boundary conversion
- `HandleInboundMessage` application module with `to_channel_message()` bridge
- All channel messages now pass through `InboundEnvelope` at dispatch boundary before `process_channel_message`
- Strangler-fig pattern: delegates to existing code, ready for gradual replacement

### Phase 4.0 Steps 5-6: Scheduled + Heartbeat via ChannelRegistryPort
- `deliver_announcement()` replaced 6-arm channel-name match with `ChannelRegistryPort.deliver()` — single OutboundIntent path for all channels
- Heartbeat delivery + deadman alerts now use ChannelRegistryPort with long-lived cached adapters
- `validate_heartbeat_channel_config()` simplified from 6-arm match to single `build_channel_by_id` call
- Signal + Mattermost added to `build_channel_by_id()` and `CachedChannelRegistry::capabilities()`
- Removed: ~110 lines of channel-name branching from scheduler, ~40 lines from heartbeat validation
- Removed: channel adapter imports (TelegramChannel, DiscordChannel, etc.) from scheduler module
- Fallback: standalone scheduler (CLI) falls back to `build_channel_by_id` when no registry available

### Phase 4.0: ChannelRegistryPort in Gateway + Channel Triage
- `ChannelRegistryPort` exposed in gateway `AppState` — web UI and REST API can now resolve channels and deliver messages
- `GET /api/channels/capabilities` — list capabilities for all known channels
- `POST /api/channels/deliver` — deliver a message to any channel via OutboundIntent (admin-only)
- Feature gate fix: `capabilities("matrix")` now correctly returns empty when compiled without `channel-matrix`
- Feature gate fix: `build_channel_by_id` error message is feature-aware
- `scrub_credentials()` on auto-reply IPC payload (security fix)
- Channel triage document (`docs/fork/channel-triage.md`): 10 Tier 1 channels to port, 17 Tier 2 deferred as tech debt

### Phase 4.0: OutboundIntent + ChannelRegistryPort (Steps 1-2)
- New `fork_core` module — fork-owned application core with ports-and-adapters architecture
- New `fork_adapters` module — infrastructure implementations of fork_core ports
- `OutboundIntent` domain type with `IntentKind`, `ChannelCapability`, `DegradationPolicy`, `RenderableContent`
- `ChannelRegistryPort` trait (`fork_core/ports/`) — resolve, capabilities, deliver
- `CachedChannelRegistry` adapter (`fork_adapters/channels/`) — long-lived cached channel adapters via `parking_lot::RwLock`; Matrix SDK client survives across deliveries
- `OutboundIntentBus` (mpsc sender/receiver) connects gateway to channels
- Push inbox processor emits `OutboundIntent` after agent::run() — IPC delegation results relay to user's channel
- Relay only fires for task/query delegation (`pending_replies` guard), not FYI text
- `scrub_credentials()` applied to both push relay text AND auto-reply IPC payload (security fix)
- Config: `push_relay_channel` + `push_relay_recipient` on `[agents_ipc]` to enable relay
- Matrix added to `build_channel_by_id` (was only telegram/discord/slack)
- `build_channel_by_id` made public for cross-module reuse

### IPC Auto-Reply Safety Net
- New `RunContext` struct (`src/agent/run_context.rs`) — shared run metadata that tracks tool executions during `agent::run()`; stepping stone toward Phase 4.0 `Run` object
- Auto-reply safety net in gateway inbox processor: when agent processes a `task`/`query` with `session_id` but never calls `agents_reply`, system automatically sends `kind=result` back to the originator — pipelines no longer hang on silent agents
- Per-session reply tracking: RunContext stores IPC tool args, checks replies per `session_id` — batch of 3 tasks correctly auto-replies only for sessions without explicit response
- Both reply paths tracked: `agents_reply` AND `agents_send(kind=result)` count as explicit replies
- Safe UTF-8 truncation for payload previews and auto-reply content (prevents panic on multi-byte chars)
- `IpcClient::send_message()` public method for gateway auto-reply delivery
- RunContext threaded through `execute_one_tool` → `run_tool_call_loop` → `agent::run()` via new `run_ctx: Option<Arc<RunContext>>` parameter
- Defensive measures: tool event cap (256), poisoned mutex recovery, unsigned auto-reply noted in logs

## 2026-03-22

### Agent Fleet: Tool Enforcement & IPC Fix
- `SYNAPSECLAW_ALLOWED_TOOLS` added to all 5 agent systemd services (was only marketing-lead)
- Per-agent tool restrictions: copywriter (file+memory), news-reader/trend-aggregator (web+memory), publisher (telegram+memory)
- Detailed per-agent SOUL.md with role-specific workflows, tool lists, IPC protocol, output formats
- Auto-generate `session_id` for `kind=task/query` in `agents_send` — fixes reply correlation bug where agents couldn't call `agents_reply` when sender omitted session_id
- Enabled `web_fetch` for news-reader and trend-aggregator (needed to read full articles)
- Dev news source paths (`docs/fork/news.md`) added to researcher/writer agent prompts

### Channel Session Intelligence (Phase 3.12)
- Rolling progressive summary for channel conversations (every 20 messages, uses cheap summary model)
- Summary injected into context on overflow — semantic preservation instead of blind truncation
- Thread context seeding — new threads receive parent conversation summary + root message (zero extra LLM cost)
- Reaction thread routing fix — emoji reactions in Matrix threads now respond in the correct thread
- New `fetch_message` Channel trait method (implemented for Matrix)
- Channel session REST API: `GET/DELETE /api/channel/sessions`, `GET /api/channel/sessions/{key}/messages`
- Channel sessions visible in web UI sidebar (read-only view, grouped by channel, delete with warning)
- `ChannelSummary` struct + `load_summary`/`save_summary`/`delete` on `SessionBackend` trait
- Concurrent summary generation prevented via in-flight dedup guard
- Atomic summary writes (tmp+rename) for crash safety
- Mutually exclusive session highlighting in sidebar (web XOR channel)
- Keyboard accessibility for channel session items

## 2026-03-21

### Tavily AI Search Integration
- Added Tavily as web search provider (Search + Extract API)
- New `tavily_extract` tool — extract content from up to 20 URLs at once (markdown/text)
- Tavily shown on Integrations page with Active/Available status
- `TAVILY_API_KEY` env var support with secret encryption

### Anthropic Theme System
- New theme system: light/dark/auto with header toggle
- Replaced old navy/blue palette with warm Anthropic-style (terracotta accent, warm cream)
- CSS custom properties across 30+ components
- Updated logo

### Telegram Post Tool
- New `telegram_post` tool — publish to Telegram channels and chats from agents
- Follows pushover/linkedin pattern: own HTTP client, SecurityPolicy rate limiting
- Token from `TELEGRAM_BOT_TOKEN` env var
- Supports HTML/Markdown/MarkdownV2 parse modes

### Bug Fixes
- Fixed white screen on load (ThemeProvider unmounted context)
- Centered modals with sidebar offset
- Added page padding to IPC pages
- Fixed SYNAPSECLAW_API_KEY for custom provider
