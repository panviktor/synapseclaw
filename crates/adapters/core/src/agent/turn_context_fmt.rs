//! Turn context formatting — adapter-layer extension point.
//!
//! Delegates to the domain-level `turn_context::format_turn_context`.
//! Exists as an adapter-layer hook if web-specific formatting
//! diverges from channels in the future. Tests live in the domain
//! crate (`turn_context.rs`).

pub use synapse_domain::application::services::turn_context::{
    FormattedTurnContext, PromptBudget, TurnMemoryContext,
};

/// Format `TurnMemoryContext` into prompt-injectable strings.
///
/// Delegates to the domain-level formatter. Adapter layer can override
/// this function if web-specific formatting needs diverge from channels.
pub fn format_turn_context(ctx: &TurnMemoryContext, budget: &PromptBudget) -> FormattedTurnContext {
    synapse_domain::application::services::turn_context::format_turn_context(ctx, budget)
}
