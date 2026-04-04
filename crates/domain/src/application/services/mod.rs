//! Application services — fork-owned business logic.
//!
//! Each service owns a domain concern and orchestrates through ports.
//! Services are the *only* place where business policy lives;
//! adapters translate, infrastructure executes.

pub mod approval_service;
pub mod bootstrap_core_memory;
pub mod channel_presentation;
pub mod conversation_service;
pub mod delivery_service;
pub mod dialogue_state_service;
pub mod history_compaction;
pub mod inbound_message_service;
pub mod ipc_service;
pub mod learning_events;
pub mod learning_signals;
pub mod loop_detection;
pub mod memory_mutation;
pub mod memory_sharing;
pub mod pipeline_service;
pub mod retention;
pub mod tool_filtering;
pub mod post_turn_orchestrator;
pub mod task_intent;
pub mod tool_middleware_service;
pub mod turn_context;
