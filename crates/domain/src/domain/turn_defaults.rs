//! Typed per-turn defaults resolved from structured runtime state.
//!
//! These defaults are not prompt text. They are deterministic runtime values
//! derived from dialogue state and user profile, then exposed to tools through
//! a scoped context port.

use crate::domain::conversation_target::ConversationDeliveryTarget;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedTurnDefaults {
    pub delivery_target: Option<ResolvedDeliveryTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedDeliveryTarget {
    pub target: ConversationDeliveryTarget,
    pub source: TurnDefaultSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnDefaultSource {
    DialogueState,
    UserProfile,
}
