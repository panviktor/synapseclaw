//! Turn context formatting — adapter-layer re-export.
//!
//! The canonical `format_turn_context` lives in the domain service
//! (`turn_context.rs`) because both web and channel paths need it.
//! This module re-exports it for adapter-layer consumers and serves
//! as a future extension point if web-specific formatting diverges.

pub use synapse_domain::application::services::turn_context::{
    FormattedTurnContext, PromptBudget, TurnMemoryContext,
};

/// Format `TurnMemoryContext` into prompt-injectable strings.
pub fn format_turn_context(ctx: &TurnMemoryContext, budget: &PromptBudget) -> FormattedTurnContext {
    synapse_domain::application::services::turn_context::format_turn_context(ctx, budget)
}
