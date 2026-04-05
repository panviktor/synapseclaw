//! Retention scoring — deterministic memory importance and decay.
//!
//! Computes a composite retention score from four signals:
//! - `relevance`: how well the entry matches recent queries (query-time)
//! - `recency`: how recently the entry was created or accessed
//! - `importance`: category-driven base importance
//! - `frequency`: how often the entry has been accessed
//!
//! The retention score guides:
//! - recall ranking (recency boost on top of BM25/vector)
//! - GC ordering (low-score entries evicted first)
//! - memory pressure decisions (what to compress/drop)

use crate::domain::memory::MemoryCategory;

// ── Retention score ──────────────────────────────────────────────

/// Composite retention score for a memory entry.
#[derive(Debug, Clone)]
pub struct RetentionScore {
    /// Semantic relevance to current query (0.0–1.0). Set at query time.
    pub relevance: f64,
    /// Time-based recency factor (0.0–1.0). Decays over time.
    pub recency: f64,
    /// Category-driven base importance (0.0–1.0).
    pub importance: f64,
    /// Access frequency factor (0.0–1.0). Grows with use.
    pub frequency: f64,
    /// Weighted composite (sum of weighted components).
    pub total: f64,
}

/// Weights for combining retention score components.
#[derive(Debug, Clone)]
pub struct RetentionWeights {
    pub relevance: f64,
    pub recency: f64,
    pub importance: f64,
    pub frequency: f64,
}

impl Default for RetentionWeights {
    fn default() -> Self {
        Self {
            relevance: 0.4,
            recency: 0.25,
            importance: 0.25,
            frequency: 0.1,
        }
    }
}

// ── Decay policy ─────────────────────────────────────────────────

/// Category-aware decay configuration.
///
/// Each category has its own half-life: the time (in hours) after which
/// the recency factor drops to 0.5. Shorter half-life = faster forgetting.
#[derive(Debug, Clone)]
pub struct RetentionPolicy {
    /// Episodic chat messages — decay fastest.
    pub conversation_half_life_hours: f64,
    /// Daily summaries — medium decay.
    pub daily_half_life_hours: f64,
    /// Reflections/lessons — slow decay.
    pub reflection_half_life_hours: f64,
    /// Core facts — very slow decay.
    pub core_half_life_hours: f64,
    /// Skills — slowest decay (procedural memory is durable).
    pub skill_half_life_hours: f64,
    /// Minimum score below which entries are GC candidates.
    pub min_keep_score: f64,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            conversation_half_life_hours: 48.0, // 2 days
            daily_half_life_hours: 168.0,       // 1 week
            reflection_half_life_hours: 720.0,  // 30 days
            core_half_life_hours: 2160.0,       // 90 days
            skill_half_life_hours: 4320.0,      // 180 days
            min_keep_score: 0.05,
        }
    }
}

impl RetentionPolicy {
    /// Get the half-life (hours) for a given memory category.
    pub fn half_life_hours(&self, category: &MemoryCategory) -> f64 {
        match category {
            MemoryCategory::Conversation => self.conversation_half_life_hours,
            MemoryCategory::Daily => self.daily_half_life_hours,
            MemoryCategory::Reflection => self.reflection_half_life_hours,
            MemoryCategory::Core => self.core_half_life_hours,
            MemoryCategory::Skill => self.skill_half_life_hours,
            MemoryCategory::Entity => self.core_half_life_hours, // entities = durable like core
            MemoryCategory::Custom(_) => self.daily_half_life_hours, // default to daily
        }
    }
}

// ── Memory pressure eviction order ───────────────────────────────

/// What gets dropped first when memory/token pressure rises.
///
/// This complements PromptBudget (which controls per-turn assembly)
/// with a storage-level priority for GC and compaction.
pub const EVICTION_PRIORITY: &[MemoryCategory] = &[
    MemoryCategory::Conversation, // 1. lowest-value ephemeral
    MemoryCategory::Daily,        // 2. summaries (can be re-derived)
    MemoryCategory::Reflection,   // 3. lessons (higher value)
    MemoryCategory::Entity,       // 4. knowledge graph nodes
    MemoryCategory::Skill,        // 5. procedural (very durable)
                                  // Core blocks are NEVER auto-evicted.
];

// ── Scoring functions ────────────────────────────────────────────

/// Compute the recency factor for an entry using exponential decay.
///
/// Returns a value in [0.0, 1.0] where 1.0 = just created, 0.5 = at half-life.
pub fn recency_factor(age_hours: f64, half_life_hours: f64) -> f64 {
    if half_life_hours <= 0.0 {
        return 0.0;
    }
    // Exponential decay: f(t) = 0.5^(t / half_life)
    (0.5_f64).powf(age_hours / half_life_hours)
}

/// Compute the frequency factor from access count.
///
/// Uses logarithmic scaling: frequent access boosts importance,
/// but with diminishing returns.
pub fn frequency_factor(access_count: u32) -> f64 {
    if access_count == 0 {
        return 0.0;
    }
    // log2(1 + count) / log2(1 + 20) — caps at ~1.0 for 20+ accesses
    let raw = (1.0 + f64::from(access_count)).log2() / (1.0 + 20.0_f64).log2();
    raw.min(1.0)
}

/// Base importance for a memory category.
///
/// Higher = more valuable by default (before access/recency adjustments).
pub fn category_importance(category: &MemoryCategory) -> f64 {
    match category {
        MemoryCategory::Core => 0.9,
        MemoryCategory::Skill => 0.85,
        MemoryCategory::Entity => 0.7,
        MemoryCategory::Reflection => 0.65,
        MemoryCategory::Daily => 0.4,
        MemoryCategory::Conversation => 0.3,
        MemoryCategory::Custom(_) => 0.5,
    }
}

/// Compute a full retention score for a memory entry.
pub fn compute_retention_score(
    relevance: f64,
    age_hours: f64,
    access_count: u32,
    category: &MemoryCategory,
    policy: &RetentionPolicy,
    weights: &RetentionWeights,
) -> RetentionScore {
    let half_life = policy.half_life_hours(category);
    let recency = recency_factor(age_hours, half_life);
    let importance = category_importance(category);
    let frequency = frequency_factor(access_count);

    let total = weights.relevance * relevance
        + weights.recency * recency
        + weights.importance * importance
        + weights.frequency * frequency;

    RetentionScore {
        relevance,
        recency,
        importance,
        frequency,
        total,
    }
}

/// Decide whether an entry should be kept based on its retention score.
pub fn should_keep(score: &RetentionScore, policy: &RetentionPolicy) -> bool {
    score.total >= policy.min_keep_score
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recency_at_zero_age_is_one() {
        let f = recency_factor(0.0, 48.0);
        assert!((f - 1.0).abs() < 0.001);
    }

    #[test]
    fn recency_at_half_life_is_half() {
        let f = recency_factor(48.0, 48.0);
        assert!((f - 0.5).abs() < 0.001);
    }

    #[test]
    fn recency_at_double_half_life_is_quarter() {
        let f = recency_factor(96.0, 48.0);
        assert!((f - 0.25).abs() < 0.001);
    }

    #[test]
    fn frequency_zero_access_is_zero() {
        assert!((frequency_factor(0) - 0.0).abs() < 0.001);
    }

    #[test]
    fn frequency_scales_logarithmically() {
        let f1 = frequency_factor(1);
        let f5 = frequency_factor(5);
        let f20 = frequency_factor(20);
        assert!(f1 > 0.0);
        assert!(f5 > f1);
        assert!(f20 > f5);
        assert!(f20 <= 1.0);
    }

    #[test]
    fn category_importance_ordering() {
        assert!(
            category_importance(&MemoryCategory::Core)
                > category_importance(&MemoryCategory::Skill)
        );
        assert!(
            category_importance(&MemoryCategory::Skill)
                > category_importance(&MemoryCategory::Entity)
        );
        assert!(
            category_importance(&MemoryCategory::Entity)
                > category_importance(&MemoryCategory::Reflection)
        );
        assert!(
            category_importance(&MemoryCategory::Reflection)
                > category_importance(&MemoryCategory::Daily)
        );
        assert!(
            category_importance(&MemoryCategory::Daily)
                > category_importance(&MemoryCategory::Conversation)
        );
    }

    #[test]
    fn half_life_conversation_fastest() {
        let p = RetentionPolicy::default();
        assert!(
            p.half_life_hours(&MemoryCategory::Conversation)
                < p.half_life_hours(&MemoryCategory::Daily)
        );
        assert!(
            p.half_life_hours(&MemoryCategory::Daily)
                < p.half_life_hours(&MemoryCategory::Reflection)
        );
        assert!(
            p.half_life_hours(&MemoryCategory::Reflection)
                < p.half_life_hours(&MemoryCategory::Core)
        );
        assert!(
            p.half_life_hours(&MemoryCategory::Core) < p.half_life_hours(&MemoryCategory::Skill)
        );
    }

    #[test]
    fn retention_score_fresh_core_fact() {
        let score = compute_retention_score(
            0.8, // high relevance
            1.0, // 1 hour old
            3,   // accessed 3 times
            &MemoryCategory::Core,
            &RetentionPolicy::default(),
            &RetentionWeights::default(),
        );
        // Fresh, relevant, important → high score
        assert!(
            score.total > 0.6,
            "expected high score, got {}",
            score.total
        );
        assert!(should_keep(&score, &RetentionPolicy::default()));
    }

    #[test]
    fn retention_score_old_conversation_noise() {
        let score = compute_retention_score(
            0.1,   // low relevance
            500.0, // ~21 days old
            0,     // never accessed
            &MemoryCategory::Conversation,
            &RetentionPolicy::default(),
            &RetentionWeights::default(),
        );
        // Old, irrelevant, ephemeral, never accessed → low score
        assert!(score.total < 0.2, "expected low score, got {}", score.total);
    }

    #[test]
    fn eviction_order_starts_with_conversation() {
        assert_eq!(EVICTION_PRIORITY[0], MemoryCategory::Conversation);
    }
}
