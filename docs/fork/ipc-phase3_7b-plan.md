# IPC Phase 3.7b: Session Intelligence & Live Events

Phase 3.7: chat sessions | **Phase 3.7b: session intelligence** | Phase 4: federated execution

---

## What Phase 3.7b gives

Three improvements to the chat session system from Phase 3.7:

1. **Rolling session summary** — LLM-generated 2-3 sentence summary updated every 10 messages. Enables resume on reconnect, meaningful sidebar labels, and future semantic search over sessions.
2. **Live tool events** — tool_call/tool_result pushed to client via WS in real-time during agent turns, not only visible after page reload.
3. **Run lifecycle events** — `session.run_started`/`run_finished`/`run_interrupted` broadcast for multi-tab typing state sync.

Bonus:
4. **Configurable summary model** — separate model for summarization (cheaper than primary), switchable on the fly from sidebar UI.
5. **Sidebar info panel** — shows active model, summary model, agent uptime.

---

## Why Phase 3.7b exists

Phase 3.7 review identified three gaps:

1. `session_summary` field existed in DB schema + ChatSession struct but was never computed or written. The resume packet was half-broken — `current_goal` worked but `session_summary` was always null.
2. Tool calls/results were persisted to durable transcript but the frontend only appended the final assistant response during `chat.send`. Tool trace was invisible until page reload.
3. Server-push only covered `session.updated`/`session.deleted`. No run lifecycle events meant other tabs couldn't sync typing state.

### Research context

Studied OpenClaw compaction (split-half + LLM summarize at 80% context), LangChain `ConversationSummaryBufferMemory`, recursive summarization ([arXiv 2308.15022](https://arxiv.org/abs/2308.15022)). Evaluated Rust crates (`llm-weaver`, `ai-agents`, `tfidf-text-summarizer`). Decided: no external deps — implement rolling summary via existing `provider.chat_with_system()`. Session summary is for **navigation/resume/search**, NOT context window management (compaction is a separate future feature).

Key insight: different from OpenClaw's approach (summarize when nearing context limit). We summarize every N messages for UX regardless of context window size — models with 1M+ context don't need compaction but still benefit from summaries.

---

## Implementation

### PR #97 — Backend: session summary, live tool events, run lifecycle

| File | Changes |
|------|---------|
| `src/config/schema.rs` | `summary_model: Option<String>` at top-level Config (next to `default_model`) |
| `src/gateway/chat_db.rs` | `update_session_summary(key, summary)` method |
| `src/gateway/ws.rs` | `summarize_session_if_needed()` — rolling LLM summary every 10 msgs |
| `src/gateway/ws.rs` | `push_tool_events()` — WS push tool_call/tool_result before rpc_response |
| `src/gateway/ws.rs` | `emit_run_event()` — session.run_started/finished/interrupted with run_id |
| `src/gateway/ws.rs` | `out_tx` passed to spawned chat.send task for live push |
| `src/gateway/mod.rs` | `summary_model: Option<String>` in AppState |
| `src/onboard/wizard.rs` | `summary_model: None` in new config constructors |
| `web/src/pages/AgentChat.tsx` | Handle tool_call/tool_result/lifecycle push events in onMessage |
| `web/src/pages/AgentChat.tsx` | Tool event rendering with muted mono styling |
| `web/src/types/api.ts` | Extended WsMessage type union with new event types |

### PR #98 — UI: sidebar info panel + live summary model switch

| File | Changes |
|------|---------|
| `src/gateway/api.rs` | `PUT /api/summary-model` endpoint for runtime switch |
| `src/gateway/api.rs` | `/api/status` now includes `summary_model` field |
| `src/gateway/ws.rs` | `summarize_session_if_needed` reads from live config (not cached AppState) |
| `web/src/components/chat/SessionSidebar.tsx` | Info panel: primary model, summary model (editable), uptime |
| `web/src/lib/api.ts` | `putSummaryModel()` API function |
| `web/src/pages/AgentChat.tsx` | Fetch status on connect, pass to sidebar, handle model switch |
| `web/src/types/api.ts` | `summary_model` field in StatusResponse |

---

## Technical Details

### Rolling Session Summary

```
Trigger: message_count % 10 == 0
Input: prev_summary + last 10 messages from DB
Model: config.summary_model (falls back to default_model)
Temperature: 0.3
Prompt: "Summarize this conversation in 2-3 sentences. Preserve: key decisions, user goals, open tasks."
Execution: fire-and-forget via tokio::spawn (doesn't delay RPC response)
Failure: log warning, keep previous summary (never blocks or loses data)
```

### Live Tool Events Protocol

```json
// Pushed via WS before rpc_response
{"type":"tool_call","session_key":"web:...","tool_name":"shell","content":"shell({\"command\":\"ls\"})","timestamp":1710600000}
{"type":"tool_result","session_key":"web:...","content":"file1.txt\nfile2.txt","timestamp":1710600001}
// Then the normal rpc_response follows
{"type":"rpc_response","id":"...","result":{"run_id":"...","response":"..."}}
```

### Run Lifecycle Events

```json
// Broadcast to all tabs in token namespace
{"type":"session.run_started","session_key":"web:...","run_id":"...","timestamp":...}
{"type":"session.run_finished","session_key":"web:...","run_id":"...","timestamp":...}
{"type":"session.run_interrupted","session_key":"web:...","run_id":"...","timestamp":...}
```

### Summary Model Configuration

```toml
# Top-level config — use a cheap model for summarization
summary_model = "deepseek/deepseek-chat"
# or local: summary_model = "ollama/llama3"
# or omit for "use primary model"
```

Runtime switch via API: `PUT /api/summary-model {"model":"deepseek/deepseek-chat"}`
Or click summary model label in sidebar → edit inline → Enter.

---

## Status

| Item | Status |
|------|--------|
| Rolling LLM session summary | Done (PR #97) |
| Live tool events via WS push | Done (PR #97) |
| Run lifecycle events | Done (PR #97) |
| Configurable summary_model | Done (PR #97 config, #98 API+UI) |
| Sidebar info panel | Done (PR #98) |
| Live summary model switch | Done (PR #98) |
| UI model selector dropdown (from available models list) | Deferred — needs `/api/models` endpoint |
| Context window compaction | Deferred — separate feature, not related to summary |
