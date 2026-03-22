# SynapseClaw News & Changelog

## 2026-03-22

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
