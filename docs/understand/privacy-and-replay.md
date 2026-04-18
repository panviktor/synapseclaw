# Privacy And Replay

Replay is useful for skill evaluation and regression checks, but not every tool call is safe to store or rerun. A replayable tool needs an explicit typed contract and sanitized arguments.

Tools that can expose private memory, session content, project context, or secrets stay excluded until typed privacy classification exists. This is why memory/session/project replay is intentionally conservative.

