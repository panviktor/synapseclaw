# Config

Main config normally lives under `~/.synapseclaw/config.toml`. Helper agent configs live under `~/.synapseclaw/agents/*/config.toml`.

Operational secrets should live outside tracked repository files, usually in `~/.config/systemd/user/synapseclaw.env`. For skills, important settings include auto-promotion policy, open-skills sources, workspace package porting, and embedding provider behavior.

