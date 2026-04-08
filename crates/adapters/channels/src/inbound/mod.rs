//! Adapters for inbound message handling ports.
//!
//! These wrap existing channels/mod.rs infrastructure to implement
//! synapse_domain ports, enabling the HandleInboundMessage orchestrator.

pub mod channel_output_adapter;
pub mod conversation_history_adapter;
pub mod conversation_store_adapter;
pub mod route_selection_adapter;
pub mod session_summary_adapter;
