# Tool Contracts Reference

Every runtime tool must provide an explicit typed protocol contract. Deprecated provider-schema metadata and `x-synapse-*` fallback extensions are not supported as runtime safety sources.

A contract should define tool role, privacy class, replay behavior, and argument policy. Replay-safe tools must expose only sanitized arguments; private memory, session, project, or secret-bearing calls remain excluded until typed privacy classification exists.

