# Config Reference

Main config lives under `~/.synapseclaw/config.toml`. Helper agents use `~/.synapseclaw/agents/*/config.toml`, and secrets should be supplied through the local service environment.

Important Skills-related config areas include auto-promotion policy, open-skills sources, and workspace package porting. Auxiliary model selection uses [model lanes](model-lanes.md); do not use legacy summary or embedding routing keys.
