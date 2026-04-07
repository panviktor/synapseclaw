//! Application services — fork-owned business logic.
//!
//! Each service owns a domain concern and orchestrates through ports.
//! Services are the *only* place where business policy lives;
//! adapters translate, infrastructure executes.

pub mod approval_service;
pub mod bootstrap_core_memory;
pub mod channel_presentation;
pub mod clarification_policy;
pub mod conversation_service;
pub mod delivery_service;
pub mod dialogue_state_service;
pub mod everyday_eval_harness;
pub mod failure_similarity_service;
pub mod history_compaction;
pub mod inbound_message_service;
pub mod ipc_service;
pub mod learning_candidate_service;
pub mod learning_conflict_service;
pub mod learning_events;
pub mod learning_evidence_service;
pub mod learning_maintenance_service;
pub mod learning_quality_service;
pub mod learning_signals;
pub mod learning_strength_service;
pub mod loop_detection;
pub mod memory_mutation;
pub mod memory_projection_service;
pub mod memory_sharing;
pub mod pipeline_service;
pub mod post_turn_orchestrator;
pub mod precedent_similarity_service;
pub mod recipe_evolution_service;
pub mod resolution_router;
pub mod retention;
pub mod retrieval_service;
pub mod self_learning_eval_harness;
pub mod skill_promotion_service;
pub mod skill_review_service;
pub mod system_event_projection_service;
pub mod tool_filtering;
pub mod tool_middleware_service;
pub mod turn_budget_policy;
pub mod turn_context;
pub mod turn_interpretation;
pub mod user_profile_service;
