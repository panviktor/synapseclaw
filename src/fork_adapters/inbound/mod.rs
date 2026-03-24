//! Adapters for inbound message handling ports.
//!
//! These wrap existing channels/mod.rs infrastructure to implement
//! fork_core ports, enabling the HandleInboundMessage orchestrator.

pub mod agent_runtime_adapter;
pub mod channel_output_adapter;
pub mod conversation_history_adapter;
pub mod hooks_adapter;
pub mod memory_adapter;
pub mod route_selection_adapter;
pub mod session_summary_adapter;
