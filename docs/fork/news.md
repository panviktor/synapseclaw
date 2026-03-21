# SynapseClaw News & Changelog

## 2026-03-21

### Tavily AI Search Integration
- Добавлен Tavily как провайдер веб-поиска (Search + Extract API)
- Новый инструмент `tavily_extract` — извлечение контента из URL (до 20 за раз, markdown/text)
- Tavily отображается на странице Integrations
- Поддержка `TAVILY_API_KEY` через env переменные

### Anthropic Theme System
- Новая система тем: light/dark/auto с переключателем в header
- Заменена старая navy/blue палитра на тёплую Anthropic-стиль (terracotta accent, warm cream)
- CSS-переменные для всех цветов (30+ компонентов обновлено)
- Обновлён логотип

### Bug Fixes
- Исправлен белый экран (ThemeProvider unmounted context)
- Модалки центрированы с учётом sidebar offset
- Паддинги на IPC страницах
- SYNAPSECLAW_API_KEY для custom provider
