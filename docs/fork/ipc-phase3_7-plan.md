# IPC Phase 3.7: Chat Sessions

Phase 3.6: agent provisioning | **Phase 3.7: chat sessions** | Phase 4: federated execution

---

## What Phase 3.7 gives

Three promises to the operator:

1. **Chat survives everything** — switch tabs, refresh page, restart daemon. Your conversation is still there.
2. **Multiple sessions** — start a new chat without losing the old one. Switch between sessions in a sidebar.
3. **Session management** — list sessions, rename them, delete old ones, see token usage per session.
4. **Durable persistence** — sessions backed by SQLite, survive daemon restarts.

---

## Why Phase 3.7 exists

The web chat (`/agent` page) currently:
- Destroys all messages on tab switch (React unmount → `useState` cleared)
- Drops the WebSocket connection on navigation (agent instance lost)
- Has no concept of sessions — one conversation per connection, no history
- Draft text persists (via `useDraft` store) but messages don't

In openclaw's UI:
- `chat.history` RPC loads conversation history from the server on every page mount
- `sessions.list` shows all recent sessions with labels, timestamps, token counts
- Users can switch between sessions, create new ones (`/new`), rename, delete
- Session key format: `agent:<agentId>:<channel>:<sender>` — structured, routable
- History survives page navigation, refresh, and even reconnection

---

## What we already have

### Backend

| Component | Location | What it does |
|-----------|----------|-------------|
| `Agent::history()` | `src/agent/agent.rs:235` | Returns `&[ConversationMessage]` — full turn history |
| `Agent::turn()` | `src/agent/agent.rs:467` | Executes one conversation turn, appends to history |
| `ConversationHistoryMap` | `src/channels/mod.rs:155` | `HashMap<String, Vec<ChatMessage>>` — per-sender history, max 50 messages |
| `conversation_history_key()` | `src/channels/mod.rs:366` | Key format: `{channel}_{thread}_{sender}` |
| WS agent | `src/gateway/ws.rs:124` | Creates `Agent` per WS connection — has multi-turn history but dies on disconnect |
| `session_store.rs` | `src/channels/session_store.rs` | New upstream module — `SessionStore` with persistence, but not wired into web chat yet |

### Frontend

| Component | Location | What it does |
|-----------|----------|-------------|
| `AgentChat` | `web/src/pages/AgentChat.tsx` | Chat page — `useState` for messages, WS for transport |
| `WebSocketClient` | `web/src/lib/ws.ts` | Connect/disconnect/send — no reconnect logic |
| `useDraft` | `web/src/hooks/useDraft.ts` | In-memory draft persistence across route changes |

### What's missing

- No WS RPC layer — can't request history or session operations over the existing WS
- WS agent dies on disconnect — history lost
- No session sidebar in web UI
- No session switching
- No session concept at all — one conversation per WS connection

---

## How openclaw does it (reference)

### Gateway RPC methods

| Method | Purpose |
|--------|---------|
| `chat.send` | Send message to agent in a session |
| `chat.history` | Load last N messages for a session |
| `chat.abort` | Cancel in-flight generation |
| `sessions.list` | List all sessions with metadata (label, updated_at, token count) |
| `sessions.patch` | Rename session, change thinking level |
| `sessions.delete` | Delete session + transcript |
| `sessions.reset` | Clear session history (keep session) |

### Session key format

```
agent:<agentId>:<channel>:<sender>
agent:main:web:user123
agent:main:telegram:987654321
agent:main:cron:daily-digest:run:abc123
```

Structured, parseable, routable. Each channel×sender pair gets its own session.

### Frontend session model

- `sessionKey` in app state — currently active session
- Session sidebar — list of recent sessions, click to switch
- `/new` command — creates new session, switches to it
- Session label — auto-generated or user-set
- Token count per session — visible in sessions list

---

## Architectural Decisions

### AD-1: Persist WS agent in AppState, keyed by session

The WS handler currently creates `Agent::from_config()` per connection — agent dies on disconnect. Instead, store agents in `AppState` keyed by session ID. On WS reconnect, reuse the existing agent with full history.

```rust
// In AppState
pub chat_sessions: Arc<Mutex<HashMap<String, ChatSession>>>,

struct ChatSession {
    agent: Agent,
    created_at: Instant,
    last_active: Instant,
    label: Option<String>,
    message_count: u32,
}
```

Eviction: agents idle > 2 hours are pruned. Max 50 sessions.

### AD-2: WS RPC for all chat/session operations

All chat and session operations go through the existing WebSocket connection as request-response RPC messages. No additional HTTP endpoints.

**Why WS, not REST**:
- WS is already open — zero connection overhead for history/session calls
- History and session switch must be instant (no HTTP roundtrip)
- openclaw uses the same pattern (`chat.history`, `sessions.list`, etc.) and it works well
- Single transport for all chat state — simpler client logic

**Protocol**: JSON messages with `type: "rpc"`, `method`, `id` (for request-response correlation), and `params`. Server responds with `type: "rpc_response"`, `id`, and `result` or `error`.

```json
// Client → Server
{ "type": "rpc", "id": "abc123", "method": "chat.history", "params": { "session": "web:a1b2:default", "limit": 50 } }

// Server → Client
{ "type": "rpc_response", "id": "abc123", "result": { "messages": [...], "session_key": "..." } }
```

**RPC methods**:

| Method | Params | Returns | Purpose |
|--------|--------|---------|---------|
| `chat.history` | `session`, `limit` | `{ messages, session_key, label }` | Load conversation history |
| `chat.send` | `session`, `message` | `{ run_id }` | Send message (replaces current raw WS send) |
| `chat.abort` | `session` | `{ ok }` | Cancel in-flight generation |
| `sessions.list` | — | `{ sessions: [{ key, label, last_active, message_count, preview }] }` | List all sessions |
| `sessions.new` | `label?` | `{ session_key, label }` | Create new session |
| `sessions.rename` | `key`, `label` | `{ ok }` | Rename session |
| `sessions.delete` | `key` | `{ ok }` | Delete session + agent |
| `sessions.reset` | `key` | `{ ok }` | Clear history, keep session |

Streaming (chunks, tool_call, done, error) stays as before — fire-and-forget server→client messages. Only request-response operations use RPC.

### AD-3: Session key = `web:<token_hash_prefix>`

For web chat sessions, the key is `web:<first-8-chars-of-token-sha256>:<session_id>`. The token hash identifies the device, the session_id differentiates multiple conversations.

Default session: `web:<hash>:default`. New sessions: `web:<hash>:<uuid-short>`.

**Key format is intentionally narrow for v1.** openclaw uses `agent:<agentId>:<channel>:<sender>` — a universal routable key that spans web, channels, cron, and subagents. Our `web:<hash>:<id>` is simpler but web-only. If we later unify web chat, channel sessions, and IPC conversations into a common session model, this key format will need migration. This is accepted for v1 — universal session identity is Phase 4 scope.

### AD-6: SessionStore for durable persistence

Sessions must survive daemon restarts. The approach:

1. **In-memory AppState** for hot sessions (fast WS RPC access, agent instances).
2. **SQLite persistence** for durable storage — on every message, write to DB. On daemon startup, hydrate AppState from DB.

The upstream `session_store.rs` (`SessionStore`) provides disk-backed session persistence but is channel-oriented (stores `ChannelMessage` with channel/sender metadata). Web chat uses `Agent` with `ConversationMessage` (role/content). Two options:

- **Option A**: Adapt `SessionStore` to also handle `ConversationMessage` — requires extending the schema.
- **Option B**: Use IPC's existing `agents.db` pattern — create a `chat_sessions` table in a new `workspace/chat/sessions.db`.

**Decision**: Option B. Simpler, no upstream module modification, follows the existing `workspace_dir/<subsystem>/<name>.db` convention. Schema:

```sql
CREATE TABLE chat_sessions (
    key         TEXT PRIMARY KEY,
    label       TEXT,
    created_at  INTEGER NOT NULL,
    last_active INTEGER NOT NULL,
    message_count INTEGER DEFAULT 0
);

CREATE TABLE chat_messages (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    session_key TEXT NOT NULL REFERENCES chat_sessions(key),
    role        TEXT NOT NULL,  -- user, assistant, system
    content     TEXT NOT NULL,
    timestamp   INTEGER NOT NULL
);
```

On daemon startup: load sessions from DB, create Agent instances, replay history into agents. On message: append to DB + in-memory. On session delete: remove from both.

### AD-4: Frontend uses session sidebar like openclaw

The chat page gets a collapsible sidebar with:
- List of sessions (label, last message preview, time ago)
- "New Chat" button
- Click to switch
- Right-click / menu for rename, delete

Active session highlighted. Session switch = `ws.rpc("chat.history", { session: newKey })`.

### AD-5: History loads on mount via WS RPC

On page mount:
1. Open WS connection (or reuse existing)
2. `ws.rpc("sessions.list")` → populate sidebar
3. `ws.rpc("chat.history", { session: activeKey, limit: 50 })` → populate messages
4. Streaming messages (chunks, tool_call, done) arrive as fire-and-forget WS messages
5. On WS reconnect after navigation → steps 2-3 again (agent still alive on server)

No REST endpoints for chat/session operations. Everything goes through WS RPC.

---

## Screens

### Chat page with session sidebar

```
┌─────────────────────────────────────────────────────────┐
│ Sidebar │ Chat                                          │
│─────────│───────────────────────────────────────────────│
│ [+ New] │                                               │
│─────────│  🤖 Agent message...                          │
│ ● Default│                                              │
│   3m ago │  👤 User message...                          │
│─────────│                                               │
│   Research│  🤖 Agent response...                       │
│   2h ago │                                               │
│─────────│                                               │
│   Debug  │                                               │
│   1d ago │  [Type a message...]              [Send]     │
└─────────────────────────────────────────────────────────┘
```

Session sidebar:
- 200px wide, collapsible (toggle button)
- "New Chat" button at top
- Sessions sorted by `last_active` descending
- Each entry: label (or "Session N"), last message preview (truncated), relative time
- Active session highlighted with blue accent
- Hover: rename (pencil icon), delete (trash icon)
- Collapse on mobile (hamburger toggle)

### Session management

- **New session**: click "+ New" → `ws.rpc("sessions.new")` → switches to empty chat
- **Switch**: click session in sidebar → `ws.rpc("chat.history", { session })` → shows messages
- **Rename**: pencil icon → inline edit → `ws.rpc("sessions.rename", { key, label })`
- **Delete**: trash icon → confirm dialog → `ws.rpc("sessions.delete", { key })`

---

## Implementation Steps

### Step 0: Backend — chat session store + SQLite persistence

**Files**: `src/gateway/mod.rs`, `src/gateway/ws.rs`, `src/gateway/chat_db.rs` (new)

**What**:
- `ChatDb` struct: SQLite DB at `workspace/chat/sessions.db`, WAL mode
- Schema: `chat_sessions` + `chat_messages` tables (see AD-6)
- `ChatSession` struct in memory: agent, created_at, last_active, label, message_count
- `chat_sessions: Arc<Mutex<HashMap<String, ChatSession>>>` in AppState
- `chat_db: Option<Arc<ChatDb>>` in AppState
- Session key derivation: `web:{token_hash_8}:{session_id}`
- WS handler: look up existing session, create if missing
- On WS disconnect: agent stays (not dropped)
- On message: write to SQLite + in-memory
- On daemon startup: hydrate sessions from SQLite, replay history into Agent instances
- Pruning: sessions idle > 24 hours evicted from memory (DB retained)
- Max 50 active sessions in memory (LRU eviction by last_active)

**Verify**: `cargo check`

---

### Step 1: Backend — WS RPC dispatcher

**Files**: `src/gateway/ws.rs`

**What**:
- Add RPC message parsing: `{ type: "rpc", id, method, params }`
- Dispatch to handler by method name
- Return `{ type: "rpc_response", id, result }` or `{ type: "rpc_response", id, error }`
- Existing raw text messages (chat send) still work for backward compat
- RPC and streaming coexist on the same WS connection

**RPC handler skeleton**:
```rust
async fn handle_rpc(method: &str, params: Value, state: &WsState) -> Result<Value> {
    match method {
        "chat.history" => handle_chat_history(params, state),
        "chat.send" => handle_chat_send(params, state),
        "chat.abort" => handle_chat_abort(params, state),
        "sessions.list" => handle_sessions_list(state),
        "sessions.new" => handle_sessions_new(params, state),
        "sessions.rename" => handle_sessions_rename(params, state),
        "sessions.delete" => handle_sessions_delete(params, state),
        "sessions.reset" => handle_sessions_reset(params, state),
        _ => Err(anyhow!("Unknown RPC method: {method}")),
    }
}
```

**Verify**: `cargo check`

---

### Step 2: Backend — RPC method implementations

**Files**: `src/gateway/ws.rs`

**What**:
- `chat.history` — read `ChatSession.agent.history()`, serialize, return
- `chat.send` — same as current raw text send but via RPC (returns run_id)
- `chat.abort` — cancel in-flight agent turn
- `sessions.list` — iterate `chat_sessions`, return metadata for sessions belonging to this token
- `sessions.new` — create new ChatSession with empty Agent, return key
- `sessions.rename` — update `ChatSession.label`
- `sessions.delete` — remove from map, drop agent
- `sessions.reset` — clear agent history, keep session

**Verify**: `cargo check`, WS client test

---

### Step 3: Frontend — session sidebar component

**Files**: `web/src/components/chat/SessionSidebar.tsx` (new)

**What**:
- Collapsible sidebar (200px, left of chat area)
- "New Chat" button
- Session list sorted by last_active
- Each entry: label, preview, time ago
- Active session highlighted
- Rename (inline edit on double-click or pencil icon)
- Delete (trash icon + confirm)
- Collapse toggle button

**Verify**: `npm run build`

---

### Step 4: Frontend — rewrite AgentChat with sessions

**Files**: `web/src/pages/AgentChat.tsx`, `web/src/lib/ws.ts`

**What**:
- Extend `WebSocketClient` with `rpc(method, params): Promise<result>` — sends RPC request, returns promise resolved by matching `rpc_response.id`
- On mount: `ws.rpc("sessions.list")` → populate sidebar, `ws.rpc("chat.history", { session, limit: 50 })` → populate messages
- Session switch: `ws.rpc("chat.history", { session: newKey })` → swap messages
- New chat: `ws.rpc("sessions.new")` → switch to empty session
- `ws.rpc("chat.send", { session, message })` replaces raw `ws.sendMessage(text)`
- Streaming (chunks, tool_call, done) still arrives as fire-and-forget
- Messages persisted on server — no loss on navigation
- Loading skeleton while RPC in flight

**Verify**: `npm run build`, manual test

---

### Step 5: Frontend — client-side cache for instant display

**Files**: `web/src/hooks/useChatStore.ts` (new)

**What**:
- Global in-memory cache (outside React lifecycle)
- Keyed by session_key: `Map<string, ChatMessage[]>`
- On mount: show cached messages instantly, then reconcile with server fetch
- On new message: add to cache + React state
- Max 20 sessions cached (LRU), max 200 messages per session
- On session delete: remove from cache

**Why**: eliminates flash of empty chat on tab switch.

**Verify**: `npm run build`

---

### Step 6: Polish

**What**:
- Scroll position restore per session
- `/new` slash command in chat input → creates new session
- `/clear` slash command → clears current session history
- Session auto-label: first user message (truncated to 40 chars)
- WS reconnection indicator ("Reconnecting...")
- Mobile: sidebar collapses to hamburger
- Keyboard: Escape closes session rename

**Verify**: full walkthrough

---

## File Structure

```
src/gateway/
├── ws.rs         # EDIT: persist agent in AppState, add RPC dispatcher + all method handlers
├── chat_db.rs    # NEW: SQLite persistence for chat sessions + messages
└── mod.rs        # EDIT: add ChatSession + ChatDb to AppState

web/src/
├── pages/
│   └── AgentChat.tsx           # EDIT: add sessions, load history via WS RPC
├── components/
│   └── chat/
│       └── SessionSidebar.tsx  # NEW: session list sidebar
├── hooks/
│   └── useChatStore.ts         # NEW: client-side message cache
└── lib/
    └── ws.ts                   # EDIT: add rpc() method for request-response over WS
```

---

## Verification

### Chat persistence
1. Send 5 messages
2. Navigate to Fleet → back to Agent
3. All 5 messages visible
4. Send 6th → works

### Session switching
5. Click "+ New Chat" → empty chat, sidebar shows 2 sessions
6. Send message in new session
7. Click old session → old messages visible
8. Click new session → new message visible

### Session management
9. Rename session → label updates in sidebar
10. Delete session → removed from sidebar, switches to next
11. Page refresh (F5) → sessions and history restored from server

### Daemon restart persistence
12. Send 3 messages → restart daemon → open chat → all 3 messages loaded from DB
13. Create 2 sessions → restart → both sessions in sidebar

### Edge cases
14. Open two browser tabs → both see same sessions
15. Send from tab 1, switch to tab 2 → message visible after refresh

---

## Risk

| Risk | Impact | Mitigation |
|------|--------|------------|
| Agent memory leak | OOM | TTL 2h + LRU 50 sessions |
| History too large | Slow load | Limit 50 messages per fetch, pagination later |
| Session key collision | Wrong history | SHA-256 prefix + UUID — negligible collision |
| WS reconnect race | Duplicate messages | Server history is source of truth, client reconciles |
| Concurrent tab edits | Stale sidebar | Polling or SSE refresh on session change |

---

## v1 vs future

| Feature | This phase | Phase 4 |
|---------|-----------|---------|
| Session persistence | SQLite-backed, survives daemon restart | — |
| Session key format | `web:<hash>:<id>` (web-only) | Universal `agent:<id>:<channel>:<sender>` |
| Multi-device | Sessions per token hash | Shared sessions across devices |
| Session export | — | Export as markdown/JSON |
| Session search | — | Full-text search |
| Branching | — | Fork a session at any point |

---

## Dependencies

**Required (done)**:
- Phase 3.5: web UI infrastructure (routes, sidebar, glass-card)
- Gateway WebSocket handler (`src/gateway/ws.rs`)
- `Agent` struct with `history()` and `turn()` methods

**Not required**:
- IPC system (chat sessions are per-instance, not inter-agent)
- Phase 3.6 (provisioning is independent)

---

## What's NOT in Phase 3.7

- File/image upload in web chat
- Markdown rendering (code blocks, tables) in chat
- Typing indicators for other users
- Session branching/forking
- Cross-device session sync
