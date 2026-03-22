# IPC Phase 3.12: Channel Session Intelligence

Phase 3.11: multi-blueprint topology | **Phase 3.12: channel session intelligence** | Phase 4.0: modular core refactor

---

## Problem

Channel conversations (Telegram, Matrix, Discord, etc.) lose semantic context when history overflows. The current `compact_sender_history` keeps the last 12 messages truncated to 600 chars — a destructive trim with no semantic preservation. Meanwhile, web chat sessions already have rolling LLM summaries (Phase 3.7b), but channels do not.

Additionally:

- **Threads start blind** — when a user creates a thread in Matrix/Discord, the bot starts with zero context from the parent conversation. Each thread is a clean slate with only memory recall, leading to disjointed responses.
- **Channel sessions are invisible** — JSONL session files exist on disk but have no API exposure and no web UI access. Operators can't view, search, or manage channel conversation history from the dashboard.
- **Reactions in threads are misrouted** — when a user reacts (emoji) to a message inside a thread, the bot's response goes to the main room instead of the thread.

No competitor has fully solved these problems. OpenClaw proposed thread context seeding (Issue #15386) but never merged it. OpenFang has cross-channel canonical sessions but no smart thread seeding. This is a differentiation opportunity.

---

## Scope

### In scope

1. Rolling progressive summary for channel conversations (port from web sessions)
2. Thread context seeding (parent summary + root message injection)
3. Channel session API endpoints (list, view messages, delete)
4. Channel sessions section in web UI sidebar (read-only view + management)
5. Reaction thread routing fix (inherit thread_ts from target message)

### Non-goals

- Cross-channel canonical sessions (Phase 4.0 territory — unified conversation store)
- Summary-based auto-tagging or topic detection
- Full-text search over channel sessions (can add later)
- Replacing the web chat session model
- Breaking changes to existing channel message processing

---

## Design

### Part A: Rolling Summary for Channels

**Summary storage**: Extend `SessionBackend` trait (`src/channels/session_backend.rs`, lines 35-79) with two new default methods:
- `load_summary(key) -> Option<ChannelSummary>` → default `None`
- `save_summary(key, summary) -> Result<()>` → default `Ok(())`

New `ChannelSummary` struct: `{ summary: String, message_count_at_summary: usize, updated_at: DateTime<Utc> }`

JSONL backend (`src/channels/session_store.rs`) stores as `{sessions_dir}/{safe_key}.summary.json` (atomic write via tmp+rename). Path sanitization reuses existing `session_path()` logic (line 27) which keeps `[a-zA-Z0-9_-]`.

**Important**: `ChannelRuntimeContext` (lines 301-339) uses concrete `session_store::SessionStore` at field `session_store: Option<Arc<session_store::SessionStore>>` (line 4162), not the trait. New methods must be implemented on `SessionStore` directly. The `SessionBackend` trait gets default no-op methods for compatibility.

**Generation**: New `summarize_channel_session_if_needed()` in `channels/mod.rs`, ported from `gateway/ws.rs:1307-1444`:
- Progressive: prompt = previous_summary + last 10 messages → 2-3 sentence summary (same prompt as ws.rs line 1366)
- Interval: every 20 messages (`CHANNEL_SUMMARY_INTERVAL = 20`) — higher than web's 10 because channel messages are less frequent
- Uses existing `[summary]` config section (`SummaryConfig` at `config/schema.rs:4189` — fields: `provider`, `model`, `temperature` (default 0.3), `api_key_env`)
- Also reads `config.summary_model` as fallback (line 88 in schema.rs)
- Fire-and-forget `tokio::spawn` after successful assistant response
- Truncate to 300 chars (same as ws.rs line 1422)

**ChannelRuntimeContext changes**: Add fields to carry summary config:
- `summary_config: Arc<SummaryConfig>` — from `config.summary`
- `summary_model: Option<String>` — from `config.summary_model`
- `provider_runtime_options` already exists (line 319) — reuse for creating summary provider

**Context overflow**: When `compact_sender_history` fires (line 2468, triggered by `is_context_window_overflow_error` at line 1104), inject stored summary as first history entry: `[Previous conversation summary: {summary}]`.

**Files**: `src/channels/session_backend.rs`, `src/channels/session_store.rs`, `src/channels/mod.rs`

### Part B: Thread Context Seeding

When first message arrives in a new thread (`!had_prior_history && thread_ts.is_some()`):

1. Load parent conversation summary (key `{channel}_{sender}` without thread_ts) — **zero LLM cost** (from Part A storage)
2. Fetch thread root message via new `Channel::fetch_message(id)` trait method — 1 API call
3. Inject as context prefix: `[Conversation summary: ...]\n[Thread started on: "..."]`

For threaded first messages, thread seeding replaces regular memory context (parent summary already captures relevant memory).

**Channel trait**: Add `async fn fetch_message(&self, message_id: &str) -> Result<Option<String>>` with default `Ok(None)` to the `Channel` trait (`src/channels/traits.rs`, lines 61-155). The trait currently has 15 methods (name, send, listen, health_check, typing, drafts, reactions, pins) — `fetch_message` becomes the 16th. Implement for Matrix first — reuse `room.event()` + JSON body extraction pattern from reaction handler (matrix.rs lines 1778-1801).

**Files**: `src/channels/traits.rs`, `src/channels/matrix.rs`, `src/channels/mod.rs`

### Part C: Channel Sessions in Web UI

**Backend**: Add `channel_session_backend: Option<Arc<dyn SessionBackend>>` to `AppState` (`src/gateway/mod.rs`, lines 322-396, currently 39 fields). Initialize in `run_gateway()` by constructing a `SessionStore` from workspace dir (same logic as channel runtime at mod.rs:4162). `AppState` already has `chat_db: Option<Arc<chat_db::ChatDb>>` (line 378) for web sessions — channel sessions are separate storage.

New REST endpoints (NOT WebSocket RPC — channel sessions are read-only, no real-time needed). Register via `.route()` chaining before `.with_state(state)` (line 1110). Existing chat routes at lines 1075-1077 for reference:
- `GET /api/channel/sessions` — list with metadata + summary
- `GET /api/channel/sessions/{key}/messages` — message history
- `DELETE /api/channel/sessions/{key}` — clear session

Reuse `CHANNEL_PREFIXES` (api.rs:841, 18 entries: telegram, discord, slack, matrix, webhook, whatsapp, mattermost, irc, lark, feishu, dingtalk, qq, nextcloud, wati, linq, clawdtalk, email, nostr) to parse channel type from session key.

Add `delete(key)` method to `SessionBackend` trait (default no-op).

**Frontend**: New "Channels" section in `SessionSidebar.tsx` (props at lines 7-21). Currently renders sessions in a flat list (lines 214-292) with no channel grouping:
- Add new section below existing sessions with divider
- Grouped by channel type (matrix/telegram/discord icons)
- Show: channel icon, sender, message count, time ago, summary snippet
- Click → read-only message view (no input box)
- Delete with confirmation warning: "This will clear conversation context"

New API functions in `web/src/lib/api.ts` (currently has NO session-related functions — web sessions use WS RPC via `sessions.list`/`sessions.new`/etc.):
- `getChannelSessions(): Promise<ChannelSessionInfo[]>`
- `getChannelSessionMessages(key: string): Promise<ChannelMessageInfo[]>`
- `deleteChannelSession(key: string): Promise<void>`

New types in `web/src/types/api.ts` (existing `ChatSessionInfo` at lines 137-148 for reference).

**Files**: `src/gateway/mod.rs`, `src/gateway/api.rs`, `src/channels/session_backend.rs`, `src/channels/session_store.rs`, `web/src/components/chat/SessionSidebar.tsx`, `web/src/pages/AgentChat.tsx`, `web/src/types/api.ts`, `web/src/lib/api.ts`

### Part D: Reaction Thread Fix

When processing a reaction (`OriginalSyncReactionEvent`, handler at matrix.rs:1746), extract `m.relates_to` thread context from the **target message** (the message being reacted to). If the target was in a thread, set `thread_ts` on the `ChannelMessage` so the bot's response goes to the correct thread.

**Status**: Code already drafted in current working tree. The reaction handler (lines 1778-1837) now extracts `thread_ts` from the target message's `m.relates_to` field and passes it through to `ChannelMessage`. Previously `thread_ts` was hardcoded to `None`. Needs to be included in PR A.

**Files**: `src/channels/matrix.rs`

---

## Steps

| # | Step | Description | Files | Depends on |
|---|------|-------------|-------|------------|
| 1 | `ChannelSummary` struct + trait methods | Add `ChannelSummary`, `load_summary`/`save_summary` to `SessionBackend` | `session_backend.rs` | — |
| 2 | JSONL summary persistence | Implement `load_summary`/`save_summary` for JSONL backend | `session_store.rs` | 1 |
| 3 | Rolling summary generation | `summarize_channel_session_if_needed()`, config wiring, fire-and-forget hook | `channels/mod.rs` | 1, 2 |
| 4 | Context overflow summary injection | Inject summary into compacted history on overflow | `channels/mod.rs` | 3 |
| 5 | `fetch_message` Channel trait | Add default method + Matrix implementation | `traits.rs`, `matrix.rs` | — |
| 6 | Thread context seeding | Inject parent summary + root message on first thread message | `channels/mod.rs` | 3, 5 |
| 7 | Reaction thread fix | Extract thread_ts from target message for reactions | `matrix.rs` | — |
| 8 | `delete` SessionBackend method | Add `delete(key)` to trait + JSONL implementation | `session_backend.rs`, `session_store.rs` | — |
| 9 | Channel session API endpoints | 3 REST endpoints on gateway | `gateway/mod.rs`, `gateway/api.rs` | 1, 8 |
| 10 | Channel sessions in web UI | Sidebar section + read-only message view | `SessionSidebar.tsx`, `AgentChat.tsx`, `api.ts` | 9 |
| 11 | Validation | fmt + clippy + test + manual Matrix testing | — | all |

---

## PR structure

| PR | Steps | Title |
|----|-------|-------|
| PR A | 1-4, 7 | `feat(channels): rolling summary + reaction thread fix` |
| PR B | 5-6 | `feat(channels): thread context seeding from parent summary` |
| PR C | 8-10 | `feat(web): channel sessions in dashboard` |
| PR D | 11 | Final validation (can merge with PR C) |

---

## Relation to Phase 4.0

Phase 3.12 delivers **immediate product value** without the Phase 4.0 architectural refactor. However, it directly feeds into Phase 4.0:

- **Rolling summary** → becomes input for Phase 4.0's `ConversationStorePort` (step 3)
- **Channel session API** → becomes the channel adapter for Phase 4.0's unified session model (step 3, 9)
- **Thread seeding** → demonstrates the capability model Phase 4.0 formalizes (step 2, 7)

Phase 4.0 should migrate these implementations behind its port abstractions rather than reimplementing them.

---

## Acceptance criteria

1. Channel conversations maintain rolling summaries that survive daemon restart.
2. New threads receive parent conversation summary + root message as context (no extra LLM call).
3. Reactions in threaded messages route responses to the correct thread.
4. Channel sessions are visible and manageable in the web dashboard.
5. No regression in existing web chat or channel functionality.
6. Summary generation uses the configured cheap summary model, not the primary model.
