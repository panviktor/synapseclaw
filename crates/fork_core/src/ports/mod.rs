//! Port definitions for the fork-owned application core.
//!
//! Ports define capabilities the core needs from the outside world.
//! Adapters (in `fork_adapters`) implement these ports.

pub mod agent_runtime;
pub mod approval;
pub mod channel_output;
pub mod coding_worker;
pub mod memory;
pub mod channel_registry;
pub mod conversation_history;
pub mod conversation_store;
pub mod hooks;
pub mod ipc_bus;
pub mod pipeline_store;
pub mod route_selection;
pub mod run_store;
pub mod session_summary;
pub mod spawn_broker;
pub mod summary;
