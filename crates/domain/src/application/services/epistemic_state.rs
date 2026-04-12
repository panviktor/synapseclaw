use crate::application::services::model_lane_resolution::{
    ResolvedModelProfile, ResolvedModelProfileConfidence, ResolvedModelProfileFreshness,
    ResolvedModelProfileSource,
};
use crate::application::services::runtime_assumptions::{
    RuntimeAssumption, RuntimeAssumptionFreshness,
};
use crate::domain::memory::{MemoryCategory, MemoryEntry};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpistemicState {
    Known,
    Inferred,
    Stale,
    Contradictory,
    NeedsVerification,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpistemicSource {
    RuntimeAssumption,
    ModelProfile,
    Memory,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EpistemicEntry {
    pub subject: String,
    pub state: EpistemicState,
    pub source: EpistemicSource,
    pub confidence_basis_points: u16,
}

pub fn epistemic_state_name(state: EpistemicState) -> &'static str {
    match state {
        EpistemicState::Known => "known",
        EpistemicState::Inferred => "inferred",
        EpistemicState::Stale => "stale",
        EpistemicState::Contradictory => "contradictory",
        EpistemicState::NeedsVerification => "needs_verification",
        EpistemicState::Unknown => "unknown",
    }
}

pub fn epistemic_source_name(source: EpistemicSource) -> &'static str {
    match source {
        EpistemicSource::RuntimeAssumption => "runtime_assumption",
        EpistemicSource::ModelProfile => "model_profile",
        EpistemicSource::Memory => "memory",
    }
}

pub fn format_epistemic_entry(entry: &EpistemicEntry) -> String {
    format!(
        "state={} source={} confidence={}",
        epistemic_state_name(entry.state),
        epistemic_source_name(entry.source),
        entry.confidence_basis_points
    )
}

pub fn epistemic_entry_for_runtime_assumption(assumption: &RuntimeAssumption) -> EpistemicEntry {
    let state = match assumption.freshness {
        RuntimeAssumptionFreshness::Challenged => EpistemicState::NeedsVerification,
        RuntimeAssumptionFreshness::CurrentTurn | RuntimeAssumptionFreshness::SessionRecent => {
            if assumption.confidence_basis_points >= 8_000 {
                EpistemicState::Known
            } else {
                EpistemicState::Inferred
            }
        }
    };

    EpistemicEntry {
        subject: assumption.value.clone(),
        state,
        source: EpistemicSource::RuntimeAssumption,
        confidence_basis_points: assumption.confidence_basis_points,
    }
}

pub fn epistemic_entry_for_model_profile(
    subject: impl Into<String>,
    profile: &ResolvedModelProfile,
) -> EpistemicEntry {
    let state = if matches!(
        profile.context_window_source,
        ResolvedModelProfileSource::Unknown
    ) {
        EpistemicState::Unknown
    } else if profile.context_window_freshness() == ResolvedModelProfileFreshness::Stale {
        EpistemicState::Stale
    } else if profile.context_window_confidence() < ResolvedModelProfileConfidence::Medium {
        EpistemicState::NeedsVerification
    } else if matches!(
        profile.context_window_source,
        ResolvedModelProfileSource::ManualConfig | ResolvedModelProfileSource::LocalOverrideCatalog
    ) {
        EpistemicState::Known
    } else {
        EpistemicState::Inferred
    };

    EpistemicEntry {
        subject: subject.into(),
        state,
        source: EpistemicSource::ModelProfile,
        confidence_basis_points: epistemic_confidence_basis_points(
            profile.context_window_confidence(),
        ),
    }
}

pub fn epistemic_entry_for_memory_entry(entry: &MemoryEntry) -> EpistemicEntry {
    let confidence = entry
        .score
        .map(score_to_basis_points)
        .unwrap_or_else(|| category_confidence_basis_points(&entry.category));
    let state = if confidence < 6_500 {
        EpistemicState::NeedsVerification
    } else if matches!(
        entry.category,
        MemoryCategory::Reflection | MemoryCategory::Conversation
    ) {
        EpistemicState::Inferred
    } else {
        EpistemicState::Known
    };

    EpistemicEntry {
        subject: entry.key.clone(),
        state,
        source: EpistemicSource::Memory,
        confidence_basis_points: confidence,
    }
}

fn epistemic_confidence_basis_points(confidence: ResolvedModelProfileConfidence) -> u16 {
    match confidence {
        ResolvedModelProfileConfidence::High => 9_000,
        ResolvedModelProfileConfidence::Medium => 7_000,
        ResolvedModelProfileConfidence::Low => 4_000,
        ResolvedModelProfileConfidence::Unknown => 0,
    }
}

fn score_to_basis_points(score: f64) -> u16 {
    (score.clamp(0.0, 1.0) * 10_000.0).round() as u16
}

fn category_confidence_basis_points(category: &MemoryCategory) -> u16 {
    match category {
        MemoryCategory::Core | MemoryCategory::Entity | MemoryCategory::Skill => 8_000,
        MemoryCategory::Daily | MemoryCategory::Reflection => 7_000,
        MemoryCategory::Conversation | MemoryCategory::Custom(_) => 6_000,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::model_lane_resolution::ResolvedModelProfileSource;

    #[test]
    fn stale_model_profile_maps_to_stale_epistemic_state() {
        let profile = ResolvedModelProfile {
            context_window_tokens: Some(128_000),
            context_window_source: ResolvedModelProfileSource::CachedProviderCatalog,
            observed_at_unix: Some(1),
            ..Default::default()
        };

        let entry = epistemic_entry_for_model_profile("openrouter:model", &profile);

        assert_eq!(entry.state, EpistemicState::Stale);
        assert_eq!(entry.source, EpistemicSource::ModelProfile);
    }

    #[test]
    fn low_score_memory_needs_verification() {
        let memory = MemoryEntry {
            id: "m1".into(),
            key: "m1".into(),
            content: "Possible but weak memory".into(),
            category: MemoryCategory::Conversation,
            timestamp: "2026-01-01T00:00:00Z".into(),
            session_id: None,
            score: Some(0.42),
        };

        let entry = epistemic_entry_for_memory_entry(&memory);

        assert_eq!(entry.state, EpistemicState::NeedsVerification);
        assert_eq!(entry.confidence_basis_points, 4_200);
    }
}
