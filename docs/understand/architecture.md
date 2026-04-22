# Architecture

Synapseclaw follows a hexagonal shape: domain services and ports define behavior, while adapters connect tools, memory, gateway, web, channels, and providers. This keeps business rules from being duplicated across transports.

Skills are the current reference implementation of this pattern. The same lifecycle and command behavior is available through CLI, gateway, web, and runtime commands instead of separate ad hoc implementations.

Recent runtime work follows the same boundary. Live-call policy, implicit memory recall, runtime watchdog logic, and usage or pressure insights live in domain services and typed ports first; Matrix, gateway, CLI, and other adapters only wire them into concrete transport or output surfaces.
