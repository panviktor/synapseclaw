//! Port definitions for the fork-owned application core.
//!
//! Ports define capabilities the core needs from the outside world.
//! Adapters (in `synapse_adapters`) implement these ports.

pub mod agent_runner;
pub mod agent_runtime;
pub mod approval;
pub mod channel;
pub mod channel_output;
pub mod channel_registry;
pub mod coding_worker;
pub mod conversation_context;
pub mod conversation_history;
pub mod conversation_store;
pub mod dead_letter;
pub mod hooks;
pub mod ipc_bus;
pub mod ipc_client;
pub mod memory;
pub mod message_router;
pub mod pipeline_executor;
pub mod pipeline_observer;
pub mod pipeline_store;
pub mod provider;
pub mod route_selection;
pub mod run_recipe_store;
pub mod run_store;
pub mod runtime;
pub mod sandbox;
pub mod session_summary;
pub mod spawn_broker;
pub mod standing_order_store;
pub mod summary;
pub mod tool;
pub mod tool_middleware;
pub mod user_profile_context;
pub mod user_profile_store;
