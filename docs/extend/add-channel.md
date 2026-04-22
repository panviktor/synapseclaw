# Add A Channel

A channel integration should deliver messages into the shared inbound/runtime command path. It should not reimplement command parsing or skill lifecycle behavior.

Channel-specific code can own transport, authentication, lifecycle, formatting constraints, and concrete side effects. Shared decisions should stay in common domain or adapter-core components.

