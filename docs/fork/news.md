# SynapseClaw News & Changelog

## 2026-03-23

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
