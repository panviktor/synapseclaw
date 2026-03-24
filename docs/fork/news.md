# SynapseClaw News & Changelog

## 2026-03-24 (2)

### Project independence: upstream detachment, i18n cleanup, README rewrite
- **Removed 29 non-EN/RU README translations** + 29 docs hub translations
- **Deleted Vietnamese docs tree** (`docs/vi/`, ~40 files) and **Chinese docs tree** (`docs/i18n/zh-CN/`, ~60 files)
- **README.md completely rewritten** — removed upstream donation links, social media badges, Special Thanks, benchmark table, Star History, contributor badges; updated project description to reflect Phase 4.0 architecture
- **README.ru.md rewritten** — mirrors new EN README in Russian
- **NOTICE updated** — minimal ZeroClaw attribution (MIT/Apache requirement)
- **Upstream sync infrastructure removed** — deleted `upstream-sync.yml` workflow, sync scripts, sync PR/issue templates
- **CONTRIBUTING.md cleaned** — removed Branch Migration Notice (upstream artifact)
- **docs/fork/README.md updated** — project is now independent, not a fork extension; removed sync automation references
- **docs/fork/sync-strategy.md archived** — kept for historical reference with archive header

## 2026-03-24

### Phase 4.0 workspace crate + all 10 use cases + full restructuring
- **fork_core extracted as workspace crate** (`crates/fork_core/`) — 0 upstream deps, compiles standalone
- Core-owned types: `ChatMessage`, `AutonomyLevel`, `HeartbeatConfig`, `CronDeliveryConfig`, `AutoDetectCandidate`
- `ChannelRegistryPort::resolve()` → `has_channel()` — removed `Channel` trait dependency
- `InboundEnvelope::from_channel_message()` moved to fork_adapters (adapter concern)
- Old `src/fork_core/` directory deleted
- **10 of 10 use cases now implemented:**
  - `SpawnChildAgent` — provision ephemeral identity, create Run(Spawn), return child token (5 tests)
  - `ResumeConversation` — load session + rebuild transcript from ConversationStorePort (4 tests)
  - `AbortConversationRun` — cancel running execution with terminal state guard (4 tests)
  - `DispatchIpcMessage` — resolve → limit → ACL → send (5 tests)
- New domain: `domain/spawn.rs` (SpawnRequest, EphemeralAgent, SpawnStatus)
- New port: `ports/spawn_broker.rs` (SpawnBrokerPort)
- **fork_adapters restructured** — `inbound/` split into `runtime/`, `memory/`, `ipc/`
- New adapters: `IpcBusAdapter`, `QuarantineAdapter` (wraps IpcDb behind ports)
- ResumeConversation **wired into ws.rs** ensure_session
- Updated progress.md, delta-registry.md, news.md
- 180 fork_core tests + 22 fork_adapters tests
- 170+ total fork_core tests

### Phase 4.0 Slice 7: CodingWorkerPort + DelegateImplementationTask
- `domain/implementation.rs` — ImplementationTask, CodingWorkerResult, ImplementationEvent, ImplementationState
- `ports/coding_worker.rs` — CodingWorkerPort (submit, poll, events, cancel)
- `delegate_implementation_task.rs` — use case: submit + track via RunStorePort + finalize
- Narrow seam: external workers are leaf executors, not replacement cores
- 158 total fork_core tests

### Phase 4.0 Slice 6: MemoryService — tier types, recall, consolidation policy
- `domain/memory.rs` — MemoryCategory, MemoryEntry, SessionMemory, RecallConfig
- `memory_service.rs` — autosave policy, recall_context() formatting, consolidation policy, tier selection
- HandleInboundMessage: `build_memory_context()` replaced with `memory_service::recall_context()`
- Deleted inline memory constants + local helper from orchestrator
- 14 unit tests (recall formatting, filtering, truncation, policy)
- 153 total fork_core tests

### Phase 4.0 Slice 5: IPC Service — domain types, ACL validation, bus port
- `domain/ipc.rs` — IpcMessage, ValidatedSend, AclError, message kind constants
- `validate_send()` — pure ACL validation (7 rules: kind, L4, direction, session, lateral)
- `validate_state_write()` / `validate_state_read()` — namespace auth (public/shared/secret)
- `ports/ipc_bus.rs` — IpcBusPort (send, fetch inbox, ack, session check, trust level)
- `ipc_service.rs` — orchestrates: resolve trust → session check → ACL validate → send via port
- gateway/ipc.rs `validate_send()` now delegates to fork_core domain function
- 28 domain tests (ACL rules + state validation) + 6 service tests
- 133 total fork_core tests

### Phase 4.0 Slice 4: ApprovalService + RequestApproval + ReviewQuarantineItem
- `domain/approval.rs` — ApprovalRequest, ApprovalResponse, ApprovalStatus, QuarantineItem, ApprovalDecision
- `ports/approval.rs` — ApprovalPort (needs_approval, request, record, session allowlist) + QuarantinePort (quarantine/promote/dismiss/list)
- `approval_service.rs` — check_needs_approval(), is_session_allowed(), create_approval_request(), quarantine operations
- `request_approval.rs` — use case: full approval workflow (session allowlist → config → port)
- `review_quarantine_item.rs` — use case: promote/dismiss/list/quarantine_agent
- `ApprovalManagerAdapter` — wraps existing ApprovalManager behind ApprovalPort
- 105 unit tests across all fork_core modules

### Phase 4.0 Slice 3: ConversationService + StartConversationRun
- `conversation_service.rs` — session key format, creation, deletion, reset, summary policy, token tracking, run lifecycle
- `start_conversation_run.rs` — use case: create run → track → finalize (success/fail/interrupt)
- Summary trigger policy: `needs_summary(count, last, interval)` — configurable per web/channel
- Summary prompt builder + truncation (300 chars max)
- Run state machine: Running → Completed | Failed | Interrupted
- 11 unit tests (6 service + 4 use case + 1 truncation)

### Phase 4.0 Slice 2: InboundMessageService — full migration + dead code removal
- channels/mod.rs reduced from 9300 → 5017 lines (−4287 lines)
- Deleted: `process_channel_message` (848 lines), 13 orphaned helper functions, 9 dead constants
- Deleted: 40+ dead tests, 8 dead test helper types
- Deleted: old `inbound_message.rs` bridge module
- Session store persistence fixed in ConversationHistoryPort adapter (JSONL survives restarts)

### Phase 4.0 Slice 2: InboundMessageService — ports + orchestrator + domain logic
- `RuntimeCommand` enum moved from channels/mod.rs to fork_core domain
- `parse_runtime_command()` — capability-driven command parsing
- `conversation_key()` — canonical key from InboundEnvelope
- `classify_message()` — command vs regular message classification
- `decide_history_enrichment()` — thread seeding vs memory context vs none
- `should_autosave()` / `should_consolidate_memory()` — memory policy
- `should_include_tool_summary()` — ToolContextDisplay capability check
- `should_interrupt_previous()` — InterruptOnNewMessage capability check
- `command_effect()` — model route resolution for runtime commands
- `CommandEffect` enum — tells adapter what state changes to make
- `HistoryEnrichment` enum — strategy pattern for first-turn context
- channels/mod.rs delegates to fork_core for all decision points
- Removed: `supports_runtime_model_switch()`, local `ChannelRuntimeCommand` enum, local `parse_runtime_command()`
- 5 new ports: `ConversationHistoryPort`, `RouteSelectionPort`, `AgentRuntimePort`, `ChannelOutputPort`, `HooksPort` + `SessionSummaryPort`
- `HandleInboundMessage` use case — full orchestration: hook → classify → route → enrich → execute → respond
- `HandleResult` enum — adapter acts on Command/Response/Cancelled
- `NoOpHooks` default implementation for hookless configurations
- 56 unit tests (services + use case)

### Phase 4.0 Slice 1: DeliveryService — first application service
- `DeliveryService` in `fork_core/application/services/delivery_service.rs` — first real application service
- Heartbeat target resolution moved from daemon/mod.rs → DeliveryService
- Auto-detect channel selection now uses `ChannelCapability::SendText` via registry (replaces hardcoded matrix>telegram>discord priority)
- Deadman alert target resolution moved to DeliveryService
- Cron delivery validation + dispatch moved from scheduler.rs → DeliveryService
- Deleted: `resolve_heartbeat_delivery()`, `auto_detect_heartbeat_channel()`, `validate_heartbeat_channel_config()` from daemon
- Deleted: `deliver_if_configured()`, `deliver_announcement()` from scheduler
- `CachedChannelRegistry` now always created in daemon mode (was IPC-only)
- 15 unit tests for delivery policy

## 2026-03-23

### Phase 4.0 Step 12: Remove transport-name branching + docs update
- 3 new `ChannelCapability` variants: `RuntimeCommands`, `InterruptOnNewMessage`, `ToolContextDisplay`
- `supports_runtime_model_switch()` → capability-driven with fallback
- Tool context summary: `msg.channel == "telegram"` → `!caps.contains(ToolContextDisplay)`
- `channel_delivery_instructions()` → `delivery_hints()` in ChannelRegistryPort (adapter metadata)
- `delivery_hints()` added to trait with default None; CachedChannelRegistry returns per-channel formatting
- `ChannelRuntimeContext` gets `channel_registry` field for capability resolution
- Progress doc updated: Steps 8-11 marked DONE, deferred items documented

### Phase 4.0 Step 11: IPC run tracking via RunStorePort
- Push-triggered IPC runs now create Run(RunOrigin::Ipc, Running) before agent execution
- Run state updated to Completed/Failed on finish
- conversation_key = `ipc:{peer_agent}` for IPC runs
- run_store passed to agent_inbox_processor via function parameter

### Phase 4.0 Step 10: Wire RunStorePort into gateway + REST API
- `RunStorePort` added to AppState, initialized from ChatDb at boot
- Web chat runs now durably persisted: create_run(Running) → update_state(Completed/Interrupted/Failed)
- REST API: `GET /api/runs` (list, optional ?conversation_key filter), `GET /api/runs/:run_id` (detail + events)
- All 3 terminal states tracked: Completed, Interrupted (user abort), Failed (error)

### Phase 4.0 Step 9: Migrate ws.rs from ChatDb to ConversationStorePort
- All 10 direct ChatDb calls in ws.rs replaced with ConversationStorePort methods
- `ensure_session`, `handle_chat_history`, `handle_sessions_*` made async for port compatibility
- `persist_message` now constructs ConversationEvent instead of ChatMessageRow
- New `replay_events_into_agent()` for Phase 4.0 path (ConversationEvent-based replay)
- MutexGuard-across-await issues resolved (history snapshot + lock release pattern)
- ChatDb remains only as fallback when conversation_store is None
- Web UI chat fully works through hexagonal architecture

### Phase 4.0 Step 8: Audit fixes + ConversationStorePort wiring + REST API
- **Audit fix**: `ConversationEvent` token fields widened from `u32` to `u64` (was silently truncating large token counts)
- **Audit fix**: `create_run` SQL now uses separate `created_at` timestamp (was reusing `started_at`)
- **Audit fix**: `delete_session` now correctly returns `false` when session didn't exist
- **Audit fix**: SQLite `PRAGMA foreign_keys = ON` added — `ON DELETE CASCADE` now enforced
- **Audit fix**: `InboundEnvelope` message IDs now use UUID v4 (was second-precision timestamp, could collide)
- `ConversationStorePort` extended with `update_label`, `update_goal`, `increment_message_count`, `add_token_usage`
- `ConversationStorePort` wired into gateway AppState (created from existing ChatDb)
- REST API: `GET /api/conversations`, `GET /api/conversations/:key`, `DELETE /api/conversations/:key`

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
