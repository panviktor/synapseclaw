//! Application use cases — top-level orchestration entry points.
//!
//! Each use case is a single operation that adapters invoke.
//! Use cases compose services and ports to implement business flows.

pub mod handle_inbound_message;
