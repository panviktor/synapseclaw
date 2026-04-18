# Architecture

Synapseclaw follows a hexagonal shape: domain services and ports define behavior, while adapters connect tools, memory, gateway, web, channels, and providers. This keeps business rules from being duplicated across transports.

Skills are the current reference implementation of this pattern. The same lifecycle and command behavior is available through CLI, gateway, web, and runtime commands instead of separate ad hoc implementations.

