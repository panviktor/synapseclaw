//! Memory mutation semantics — AUDN-lite decision model.
//!
//! Defines what it means to change long-term memory: Add, Update, Delete, or Noop.
//! The mutation service uses these types to make deterministic decisions about
//! whether extracted facts should be appended, merged, replaced, or discarded.

use super::memory::MemoryCategory;

// ── Mutation actions ─────────────────────────────────────────────

/// What to do with a candidate memory fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationAction {
    /// New information — no similar existing memory found.
    Add,
    /// Existing memory should be updated/replaced.
    Update { target_id: String },
    /// Existing memory should be removed (contradiction or retraction).
    Delete { target_id: String },
    /// No action needed — duplicate or irrelevant.
    Noop,
}

impl MutationAction {
    pub fn is_noop(&self) -> bool {
        matches!(self, Self::Noop)
    }
}

// ── Candidate ────────────────────────────────────────────────────

/// A candidate fact extracted from a turn, ready for mutation evaluation.
#[derive(Debug, Clone)]
pub struct MutationCandidate {
    /// Which memory category this fact belongs to.
    pub category: MemoryCategory,
    /// The fact text to evaluate.
    pub text: String,
    /// Confidence of the extraction (0.0–1.0).
    pub confidence: f32,
    /// Where this candidate came from.
    pub source: MutationSource,
    /// Explicit memory-quality class used by the governor before durable writes.
    pub write_class: Option<MutationWriteClass>,
}

/// Origin of a mutation candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationSource {
    /// Background consolidation extracted this fact.
    Consolidation,
    /// Pre-compress handoff extracted this fact from a soon-to-be-dropped range.
    PreCompressHandoff,
    /// User explicitly stated this (hot-path).
    ExplicitUser,
    /// Tool/knowledge graph wrote this.
    ToolOutput,
    /// Reflection/skill learning produced this.
    Reflection,
}

/// Durable write class for memory-quality governance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationWriteClass {
    Preference,
    TaskState,
    FactAnchor,
    Recipe,
    FailurePattern,
    EphemeralRepairTrace,
    GenericDialogue,
}

// ── Decision ─────────────────────────────────────────────────────

/// The result of evaluating a mutation candidate against existing memory.
#[derive(Debug, Clone)]
pub struct MutationDecision {
    /// What action to take.
    pub action: MutationAction,
    /// The candidate that was evaluated.
    pub candidate: MutationCandidate,
    /// Human-readable reason for the decision.
    pub reason: String,
    /// Similarity score of the closest match (if any).
    pub similarity: Option<f64>,
}

// ── Thresholds ───────────────────────────────────────────────────

/// Configurable thresholds for AUDN-lite similarity matching.
#[derive(Debug, Clone)]
pub struct MutationThresholds {
    /// Above this: near-exact duplicate → Noop.
    pub noop_threshold: f64,
    /// Above this (+ same category): contradictory or update → Update/Delete.
    pub update_threshold: f64,
    // Below update_threshold: new fact → Add (implicit).
}

impl Default for MutationThresholds {
    fn default() -> Self {
        Self {
            noop_threshold: 0.95,
            update_threshold: 0.85,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_detected() {
        assert!(MutationAction::Noop.is_noop());
        assert!(!MutationAction::Add.is_noop());
        assert!(!MutationAction::Update {
            target_id: "x".into()
        }
        .is_noop());
    }

    #[test]
    fn default_thresholds() {
        let t = MutationThresholds::default();
        assert!((t.noop_threshold - 0.95).abs() < f64::EPSILON);
        assert!((t.update_threshold - 0.85).abs() < f64::EPSILON);
    }
}
