//! Application use cases — top-level orchestration entry points.
//!
//! Each use case is a single operation that adapters invoke.
//! Use cases compose services and ports to implement business flows.

pub mod abort_conversation_run;
pub mod delegate_implementation_task;
pub mod dispatch_ipc_message;
pub mod handle_inbound_message;
pub mod request_approval;
pub mod resume_conversation;
pub mod review_quarantine_item;
pub mod spawn_child_agent;
pub mod start_conversation_run;
