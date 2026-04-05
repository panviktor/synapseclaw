//! Memory sharing policy — namespace-aware visibility promotion.
//!
//! Enforces ownership and promotion rules for multi-agent memory:
//! - Only the owner can promote visibility
//! - Cannot demote (Private → Shared/Global only, not the reverse)
//! - SharedWith can be widened but not narrowed
//! - Conflict resolution: authority > recency > confidence

use crate::domain::memory::{AgentId, MemoryError, MemoryId, Visibility};

// ── Promotion ────────────────────────────────────────────────────

/// A validated visibility promotion request.
#[derive(Debug, Clone)]
pub struct VisibilityPromotion {
    pub entry_id: MemoryId,
    pub from: Visibility,
    pub to: Visibility,
    pub promoted_by: AgentId,
}

/// Validate a promotion request.
///
/// Policy:
/// - Only the owner can promote.
/// - Cannot demote (Global → Private is invalid).
/// - SharedWith can add agents but not remove.
/// - Private → SharedWith/Global is valid.
/// - SharedWith → Global is valid.
pub fn validate_promotion(
    entry_id: &MemoryId,
    current_visibility: &Visibility,
    current_owner: &AgentId,
    requesting_agent: &AgentId,
    target_visibility: &Visibility,
) -> Result<VisibilityPromotion, MemoryError> {
    // Only the owner can promote
    if current_owner != requesting_agent {
        return Err(MemoryError::AccessDenied {
            agent: requesting_agent.clone(),
            action: "promote_visibility".into(),
            resource: entry_id.clone(),
        });
    }

    // Validate promotion direction (no demotion)
    let valid = match (current_visibility, target_visibility) {
        // Private can go anywhere
        (Visibility::Private, Visibility::SharedWith(_)) => true,
        (Visibility::Private, Visibility::Global) => true,
        // SharedWith can go to Global or widen
        (Visibility::SharedWith(_), Visibility::Global) => true,
        (Visibility::SharedWith(old), Visibility::SharedWith(new)) => {
            // Must be a superset (widening only)
            old.iter().all(|a| new.contains(a))
        }
        // Same visibility is a no-op (allowed but pointless)
        (a, b) if a == b => true,
        // Everything else is a demotion
        _ => false,
    };

    if !valid {
        return Err(MemoryError::AccessDenied {
            agent: requesting_agent.clone(),
            action: format!("demote {:?} → {:?}", current_visibility, target_visibility),
            resource: entry_id.clone(),
        });
    }

    Ok(VisibilityPromotion {
        entry_id: entry_id.clone(),
        from: current_visibility.clone(),
        to: target_visibility.clone(),
        promoted_by: requesting_agent.clone(),
    })
}

// ── Conflict resolution ──────────────────────────────────────────

/// Resolution decision for conflicting shared facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictResolution {
    /// Keep the existing fact unchanged.
    KeepExisting,
    /// Replace with the incoming fact.
    ReplaceWithIncoming,
    /// Keep both facts (ambiguous, let user resolve).
    KeepBoth,
}

/// Resolve a conflict between two facts using deterministic rules.
///
/// Resolution order: authority > recency > confidence.
/// No LLM arbitration — pure domain logic.
pub fn resolve_conflict(
    existing_owner: &AgentId,
    existing_confidence: f64,
    existing_timestamp: i64,
    incoming_owner: &AgentId,
    incoming_confidence: f64,
    incoming_timestamp: i64,
    authoritative_agent: Option<&AgentId>,
) -> ConflictResolution {
    // 1. Authority: if one agent is authoritative, it wins
    if let Some(auth) = authoritative_agent {
        if incoming_owner == auth && existing_owner != auth {
            return ConflictResolution::ReplaceWithIncoming;
        }
        if existing_owner == auth && incoming_owner != auth {
            return ConflictResolution::KeepExisting;
        }
    }

    // 2. Recency: newer fact wins (timestamp in seconds)
    if incoming_timestamp > existing_timestamp + 60 {
        return ConflictResolution::ReplaceWithIncoming;
    }
    if existing_timestamp > incoming_timestamp + 60 {
        return ConflictResolution::KeepExisting;
    }

    // 3. Confidence: higher confidence wins
    if incoming_confidence > existing_confidence + 0.05 {
        return ConflictResolution::ReplaceWithIncoming;
    }
    if existing_confidence > incoming_confidence + 0.05 {
        return ConflictResolution::KeepExisting;
    }

    // 4. Tie: keep both
    ConflictResolution::KeepBoth
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Promotion tests ──

    #[test]
    fn owner_can_promote_private_to_shared() {
        let result = validate_promotion(
            &"entry1".to_string(),
            &Visibility::Private,
            &"agent-a".to_string(),
            &"agent-a".to_string(),
            &Visibility::SharedWith(vec!["agent-b".to_string()]),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn owner_can_promote_private_to_global() {
        let result = validate_promotion(
            &"entry1".to_string(),
            &Visibility::Private,
            &"agent-a".to_string(),
            &"agent-a".to_string(),
            &Visibility::Global,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn non_owner_cannot_promote() {
        let result = validate_promotion(
            &"entry1".to_string(),
            &Visibility::Private,
            &"agent-a".to_string(),
            &"agent-b".to_string(), // not owner
            &Visibility::Global,
        );
        assert!(result.is_err());
    }

    #[test]
    fn cannot_demote_global_to_private() {
        let result = validate_promotion(
            &"entry1".to_string(),
            &Visibility::Global,
            &"agent-a".to_string(),
            &"agent-a".to_string(),
            &Visibility::Private,
        );
        assert!(result.is_err());
    }

    #[test]
    fn shared_can_widen_but_not_narrow() {
        // Widen: [b] → [b, c] ✓
        let result = validate_promotion(
            &"entry1".to_string(),
            &Visibility::SharedWith(vec!["b".to_string()]),
            &"a".to_string(),
            &"a".to_string(),
            &Visibility::SharedWith(vec!["b".to_string(), "c".to_string()]),
        );
        assert!(result.is_ok());

        // Narrow: [b, c] → [b] ✗
        let result = validate_promotion(
            &"entry1".to_string(),
            &Visibility::SharedWith(vec!["b".to_string(), "c".to_string()]),
            &"a".to_string(),
            &"a".to_string(),
            &Visibility::SharedWith(vec!["b".to_string()]),
        );
        assert!(result.is_err());
    }

    // ── Conflict resolution tests ──

    #[test]
    fn authority_wins() {
        let result = resolve_conflict(
            &"agent-a".to_string(),
            0.9,
            1000,
            &"agent-b".to_string(),
            0.5,
            900,
            Some(&"agent-b".to_string()), // agent-b is authoritative
        );
        assert_eq!(result, ConflictResolution::ReplaceWithIncoming);
    }

    #[test]
    fn recency_wins_without_authority() {
        let result = resolve_conflict(
            &"a".to_string(),
            0.8,
            1000,
            &"b".to_string(),
            0.8,
            2000, // much newer
            None,
        );
        assert_eq!(result, ConflictResolution::ReplaceWithIncoming);
    }

    #[test]
    fn confidence_wins_on_tie() {
        let result = resolve_conflict(
            &"a".to_string(),
            0.5,
            1000,
            &"b".to_string(),
            0.9,
            1000, // same time, higher confidence
            None,
        );
        assert_eq!(result, ConflictResolution::ReplaceWithIncoming);
    }

    #[test]
    fn keep_both_on_full_tie() {
        let result = resolve_conflict(
            &"a".to_string(),
            0.8,
            1000,
            &"b".to_string(),
            0.8,
            1000, // same everything
            None,
        );
        assert_eq!(result, ConflictResolution::KeepBoth);
    }
}
