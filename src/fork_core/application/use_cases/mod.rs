//! Application use cases — top-level orchestration entry points.
//!
//! Each use case is a single operation that adapters invoke.
//! Use cases compose services and ports to implement business flows.

pub mod handle_inbound_message;
pub mod request_approval;
pub mod review_quarantine_item;
pub mod start_conversation_run;
