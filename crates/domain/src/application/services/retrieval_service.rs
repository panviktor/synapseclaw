//! Shared retrieval service for everyday assistant context lookup.
//!
//! This is the first Phase 4.8 retrieval backbone slice: tools and runtime
//! should reuse one application-level search implementation instead of
//! duplicating ad-hoc scoring logic.

use crate::domain::conversation::{ConversationEvent, ConversationKind, EventType};
use crate::domain::memory::{
    Entity, MemoryEntry, MemoryError, MemoryQuery, Skill, SkillOrigin, SkillStatus,
};
use crate::domain::run_recipe::RunRecipe;
use crate::ports::conversation_store::ConversationStorePort;
use crate::ports::memory::UnifiedMemoryPort;
use crate::ports::run_recipe_store::RunRecipeStorePort;
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq)]
pub struct SessionSearchMatch {
    pub score: f64,
    pub session_key: String,
    pub label: Option<String>,
    pub kind: ConversationKind,
    pub message_count: u32,
    pub summary: Option<String>,
    pub recap: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MemorySearchMatch {
    pub entry: MemoryEntry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRecipeSearchMatch {
    pub score: i64,
    pub task_family: String,
    pub sample_request: String,
    pub summary: String,
    pub tool_pattern: Vec<String>,
    pub success_count: u32,
    pub updated_at: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionSearchWeights {
    pub metadata: f64,
    pub transcript: f64,
    pub semantic: f64,
    pub recency: f64,
}

impl Default for SessionSearchWeights {
    fn default() -> Self {
        Self {
            metadata: 1.0,
            transcript: 1.0,
            semantic: 4.0,
            recency: 1.2,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionSearchOptions {
    pub kind_filter: Option<String>,
    pub limit: usize,
    pub metadata_shortlist: usize,
    pub recent_shortlist: usize,
    pub transcript_shortlist: usize,
    pub semantic_shortlist: usize,
    pub min_score: f64,
    pub recency_half_life_secs: u64,
    pub mmr_lambda: f64,
    pub weights: SessionSearchWeights,
}

impl Default for SessionSearchOptions {
    fn default() -> Self {
        Self {
            kind_filter: None,
            limit: 5,
            metadata_shortlist: 15,
            recent_shortlist: 25,
            transcript_shortlist: 18,
            semantic_shortlist: 16,
            min_score: 0.0,
            recency_half_life_secs: 7 * 24 * 60 * 60,
            mmr_lambda: 0.72,
            weights: SessionSearchWeights::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunRecipeSearchWeights {
    pub lexical: f64,
    pub semantic: f64,
    pub success: f64,
    pub recency: f64,
}

impl Default for RunRecipeSearchWeights {
    fn default() -> Self {
        Self {
            lexical: 1.0,
            semantic: 4.0,
            success: 1.0,
            recency: 0.8,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunRecipeSearchOptions {
    pub limit: usize,
    pub min_score: f64,
    pub recency_half_life_secs: u64,
    pub mmr_lambda: f64,
    pub lexical_shortlist: usize,
    pub recent_shortlist: usize,
    pub success_shortlist: usize,
    pub weights: RunRecipeSearchWeights,
}

impl Default for RunRecipeSearchOptions {
    fn default() -> Self {
        Self {
            limit: 5,
            min_score: 0.0,
            recency_half_life_secs: 14 * 24 * 60 * 60,
            mmr_lambda: 0.72,
            lexical_shortlist: 24,
            recent_shortlist: 24,
            success_shortlist: 16,
            weights: RunRecipeSearchWeights::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HybridTurnSearchOptions {
    pub recall_max_entries: usize,
    pub recall_min_relevance: f64,
    pub skills_max_count: usize,
    pub skills_total_max_chars: usize,
    pub entities_max_count: usize,
    pub entities_total_max_chars: usize,
    pub query_limit: usize,
}

#[derive(Debug, Clone, Default)]
pub struct HybridTurnSearchMatch {
    pub recalled_entries: Vec<MemoryEntry>,
    pub skills: Vec<Skill>,
    pub entities: Vec<Entity>,
}

pub async fn search_sessions(
    memory: &dyn UnifiedMemoryPort,
    store: &dyn ConversationStorePort,
    query: &str,
    kind_filter: Option<&str>,
    limit: usize,
) -> Vec<SessionSearchMatch> {
    let options = SessionSearchOptions {
        kind_filter: kind_filter.map(ToOwned::to_owned),
        limit,
        ..SessionSearchOptions::default()
    };
    search_sessions_with_options(memory, store, query, &options).await
}

pub async fn search_sessions_with_options(
    memory: &dyn UnifiedMemoryPort,
    store: &dyn ConversationStorePort,
    query: &str,
    options: &SessionSearchOptions,
) -> Vec<SessionSearchMatch> {
    let query_lower = query.to_lowercase();
    let keywords: Vec<&str> = query_lower.split_whitespace().collect();
    let kind_filter = options.kind_filter.as_deref();

    let mut sessions = store.list_sessions(None).await;
    sessions.sort_by(|a, b| b.last_active.cmp(&a.last_active));

    let mut metadata_hits: Vec<(f64, crate::domain::conversation::ConversationSession)> = sessions
        .iter()
        .filter(|session| matches_kind_filter(session, kind_filter))
        .filter_map(|session| {
            let score = score_session_metadata(&keywords, session) * options.weights.metadata;

            if score > 0.0 {
                Some((score, session.clone()))
            } else {
                None
            }
        })
        .collect();

    metadata_hits.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    let mut shortlisted = Vec::new();
    for (_, session) in metadata_hits.iter().take(options.metadata_shortlist) {
        if !shortlisted.iter().any(
            |existing: &crate::domain::conversation::ConversationSession| {
                existing.key == session.key
            },
        ) {
            shortlisted.push(session.clone());
        }
    }
    for session in sessions
        .iter()
        .filter(|session| matches_kind_filter(session, kind_filter))
        .take(options.recent_shortlist)
    {
        if !shortlisted
            .iter()
            .any(|existing| existing.key == session.key)
        {
            shortlisted.push(session.clone());
        }
    }

    let now = current_unix_seconds();
    let transcript_keys = if !metadata_hits.is_empty() {
        let mut ranked = shortlisted
            .iter()
            .map(|session| {
                let base_score = metadata_hits
                    .iter()
                    .find(|(_, candidate)| candidate.key == session.key)
                    .map(|(score, _)| *score)
                    .unwrap_or(0.0);
                let recency_score = temporal_decay_score(
                    now.saturating_sub(session.last_active),
                    options.recency_half_life_secs,
                ) * options.weights.recency;
                (base_score + recency_score, session.key.clone())
            })
            .collect::<Vec<_>>();
        ranked.sort_by(|left, right| {
            right
                .0
                .partial_cmp(&left.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.1.cmp(&right.1))
        });
        ranked
            .into_iter()
            .take(options.transcript_shortlist)
            .map(|(_, key)| key)
            .collect::<HashSet<_>>()
    } else {
        shortlisted
            .iter()
            .map(|session| session.key.clone())
            .collect::<HashSet<_>>()
    };

    let query_embedding = embed_query_or_none(memory, query).await;
    let mut candidates = Vec::new();
    for session in shortlisted {
        let base_score = metadata_hits
            .iter()
            .find(|(_, candidate)| candidate.key == session.key)
            .map(|(score, _)| *score)
            .unwrap_or(0.0);
        let events = if transcript_keys.contains(session.key.as_str()) {
            store.get_events(&session.key, 20).await
        } else {
            Vec::new()
        };
        let (transcript_score, recap) = transcript_recap(&events, &keywords);
        let document = build_session_document(&session, &events);
        let recency_score = temporal_decay_score(
            now.saturating_sub(session.last_active),
            options.recency_half_life_secs,
        ) * options.weights.recency;
        let transcript_component = transcript_score * options.weights.transcript;
        let cheap_score = base_score + transcript_component + recency_score;

        candidates.push(SessionSearchCandidate {
            session,
            recap,
            document,
            cheap_score,
            recency_score,
            transcript_component,
            base_score,
        });
    }

    let semantic_keys = if query_embedding.is_some() && !metadata_hits.is_empty() {
        let mut ranked = candidates
            .iter()
            .map(|candidate| (candidate.cheap_score, candidate.session.key.clone()))
            .collect::<Vec<_>>();
        ranked.sort_by(|left, right| {
            right
                .0
                .partial_cmp(&left.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.1.cmp(&right.1))
        });
        ranked
            .into_iter()
            .take(options.semantic_shortlist)
            .map(|(_, key)| key)
            .collect::<HashSet<_>>()
    } else {
        candidates
            .iter()
            .map(|candidate| candidate.session.key.clone())
            .collect::<HashSet<_>>()
    };

    let mut hits = Vec::new();
    for candidate in candidates {
        let document_embedding = if query_embedding.is_some()
            && semantic_keys.contains(candidate.session.key.as_str())
        {
            embed_document_or_none(memory, &candidate.document).await
        } else {
            None
        };
        let semantic_score = if let Some(query_embedding) = query_embedding.as_deref() {
            document_embedding
                .as_deref()
                .and_then(|doc_embedding| cosine_similarity(query_embedding, doc_embedding))
                .map(|score| score * options.weights.semantic)
                .unwrap_or(0.0)
        } else {
            0.0
        };
        let total = candidate.base_score
            + candidate.transcript_component
            + semantic_score
            + candidate.recency_score;
        if total > options.min_score {
            hits.push(RankedCandidate {
                score: total,
                embedding: document_embedding,
                item: SessionSearchMatch {
                    score: total,
                    session_key: candidate.session.key.clone(),
                    label: candidate.session.label.clone(),
                    kind: candidate.session.kind.clone(),
                    message_count: candidate.session.message_count,
                    summary: candidate.session.summary.clone(),
                    recap: candidate.recap,
                },
            });
        }
    }

    select_with_mmr(hits, options.limit, options.mmr_lambda)
}

pub async fn search_memory(
    memory: &dyn UnifiedMemoryPort,
    query: &str,
    limit: usize,
    session_id: Option<&str>,
) -> Result<Vec<MemorySearchMatch>, MemoryError> {
    let entries = memory.recall(query, limit, session_id).await?;
    Ok(entries
        .into_iter()
        .map(|entry| MemorySearchMatch { entry })
        .collect())
}

pub async fn search_run_recipes(
    memory: &dyn UnifiedMemoryPort,
    store: &dyn RunRecipeStorePort,
    agent_id: &str,
    query: &str,
    limit: usize,
) -> Vec<RunRecipeSearchMatch> {
    let options = RunRecipeSearchOptions {
        limit,
        ..RunRecipeSearchOptions::default()
    };
    search_run_recipes_with_options(memory, store, agent_id, query, &options).await
}

pub async fn search_run_recipes_with_options(
    memory: &dyn UnifiedMemoryPort,
    store: &dyn RunRecipeStorePort,
    agent_id: &str,
    query: &str,
    options: &RunRecipeSearchOptions,
) -> Vec<RunRecipeSearchMatch> {
    let query_lower = query.to_lowercase();
    let keywords: Vec<&str> = query_lower.split_whitespace().collect();
    let query_embedding = embed_query_or_none(memory, query).await;
    let now = current_unix_seconds();
    let mut recipes = store.list(agent_id);

    recipes.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| right.success_count.cmp(&left.success_count))
            .then_with(|| left.task_family.cmp(&right.task_family))
    });

    let lexical_scores = recipes
        .iter()
        .filter_map(|recipe| {
            let score = score_recipe_keywords(recipe, &keywords);
            if score > 0.0 {
                Some((score, recipe.task_family.clone()))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    let mut lexical_scores = lexical_scores;
    lexical_scores.sort_by(|left, right| {
        right
            .0
            .partial_cmp(&left.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.1.cmp(&right.1))
    });

    let mut shortlisted = Vec::new();
    for (_, family) in lexical_scores.iter().take(options.lexical_shortlist) {
        if let Some(recipe) = recipes.iter().find(|recipe| recipe.task_family == *family) {
            if !shortlisted
                .iter()
                .any(|existing: &RunRecipe| existing.task_family == recipe.task_family)
            {
                shortlisted.push(recipe.clone());
            }
        }
    }
    for recipe in recipes.iter().take(options.recent_shortlist) {
        if !shortlisted
            .iter()
            .any(|existing| existing.task_family == recipe.task_family)
        {
            shortlisted.push(recipe.clone());
        }
    }
    let mut success_sorted = recipes.clone();
    success_sorted.sort_by(|left, right| {
        right
            .success_count
            .cmp(&left.success_count)
            .then_with(|| right.updated_at.cmp(&left.updated_at))
            .then_with(|| left.task_family.cmp(&right.task_family))
    });
    for recipe in success_sorted.iter().take(options.success_shortlist) {
        if !shortlisted
            .iter()
            .any(|existing| existing.task_family == recipe.task_family)
        {
            shortlisted.push(recipe.clone());
        }
    }

    let mut hits = Vec::new();
    for recipe in shortlisted {
        let lexical = score_recipe_keywords(&recipe, &keywords) * options.weights.lexical;
        let document = build_recipe_document(&recipe);
        let document_embedding = if query_embedding.is_some() {
            embed_document_or_none(memory, &document).await
        } else {
            None
        };
        let semantic = if let Some(query_embedding) = query_embedding.as_deref() {
            document_embedding
                .as_deref()
                .and_then(|doc_embedding| cosine_similarity(query_embedding, doc_embedding))
                .map(|score| score * options.weights.semantic)
                .unwrap_or(0.0)
        } else {
            0.0
        };
        let success_bonus = (recipe.success_count.min(10) as f64) * 0.2 * options.weights.success;
        let recency_score = temporal_decay_score(
            now.saturating_sub(recipe.updated_at),
            options.recency_half_life_secs,
        ) * options.weights.recency;
        let total = lexical + semantic + success_bonus + recency_score;
        if total <= options.min_score {
            continue;
        }
        hits.push(RankedCandidate {
            score: total,
            embedding: document_embedding,
            item: RunRecipeSearchMatch {
                score: (total * 100.0).round() as i64,
                task_family: recipe.task_family,
                sample_request: recipe.sample_request,
                summary: recipe.summary,
                tool_pattern: recipe.tool_pattern,
                success_count: recipe.success_count,
                updated_at: recipe.updated_at,
            },
        });
    }

    select_with_mmr(hits, options.limit, options.mmr_lambda)
}

pub async fn search_turn_hybrid(
    memory: &dyn UnifiedMemoryPort,
    query_text: &str,
    agent_id: &str,
    session_id: Option<&str>,
    options: &HybridTurnSearchOptions,
) -> Result<HybridTurnSearchMatch, MemoryError> {
    let query = MemoryQuery {
        text: query_text.to_string(),
        embedding: None,
        agent_id: agent_id.to_string(),
        categories: Vec::new(),
        include_shared: false,
        time_range: None,
        limit: options.query_limit,
    };

    let result = memory.hybrid_search(&query).await?;
    let mut matched = HybridTurnSearchMatch::default();

    for scored in result.episodes {
        if matched.recalled_entries.len() >= options.recall_max_entries {
            break;
        }
        let mut entry = scored.entry;
        if session_id.is_some_and(|sid| entry.session_id.as_deref() != Some(sid)) {
            continue;
        }
        if entry.key.trim().is_empty() || is_autosave_key(&entry.key) {
            continue;
        }
        if crate::domain::util::should_skip_autosave_content(&entry.content) {
            continue;
        }
        if entry.content.contains("<tool_result") {
            continue;
        }
        if scored.score < options.recall_min_relevance as f32 {
            continue;
        }
        entry.score = Some(scored.score as f64);
        matched.recalled_entries.push(entry);
    }

    let mut skills = result.skills;
    skills.retain(skill_is_runtime_active);
    skills.sort_by(compare_skills_for_runtime);

    let mut skill_chars = 0usize;
    for skill in skills {
        if matched.skills.len() >= options.skills_max_count {
            break;
        }
        if skill.content.trim().is_empty() {
            continue;
        }
        let len = skill.content.chars().count();
        if skill_chars + len > options.skills_total_max_chars {
            break;
        }
        skill_chars += len;
        matched.skills.push(skill);
    }

    let mut entity_chars = 0usize;
    for entity in result.entities {
        if matched.entities.len() >= options.entities_max_count {
            break;
        }
        let summary_len = entity.summary.as_ref().map_or(0, |s| s.chars().count());
        if summary_len == 0 {
            continue;
        }
        if entity_chars + summary_len > options.entities_total_max_chars {
            break;
        }
        entity_chars += summary_len;
        matched.entities.push(entity);
    }

    Ok(matched)
}

fn skill_is_runtime_active(skill: &Skill) -> bool {
    matches!(skill.status, SkillStatus::Active)
}

fn compare_skills_for_runtime(left: &Skill, right: &Skill) -> std::cmp::Ordering {
    skill_origin_priority(&right.origin)
        .cmp(&skill_origin_priority(&left.origin))
        .then_with(|| right.success_count.cmp(&left.success_count))
        .then_with(|| right.updated_at.cmp(&left.updated_at))
        .then_with(|| left.name.cmp(&right.name))
}

fn skill_origin_priority(origin: &SkillOrigin) -> u8 {
    match origin {
        SkillOrigin::Manual => 3,
        SkillOrigin::Imported => 2,
        SkillOrigin::Learned => 1,
    }
}

fn is_autosave_key(key: &str) -> bool {
    let normalized = key.trim().to_ascii_lowercase();
    normalized == "assistant_resp" || normalized.starts_with("assistant_resp_")
}

#[derive(Debug, Clone)]
struct RankedCandidate<T> {
    score: f64,
    embedding: Option<Vec<f32>>,
    item: T,
}

#[derive(Debug, Clone)]
struct SessionSearchCandidate {
    session: crate::domain::conversation::ConversationSession,
    recap: Option<String>,
    document: String,
    cheap_score: f64,
    recency_score: f64,
    transcript_component: f64,
    base_score: f64,
}

fn score_text(keywords: &[&str], text: &str, weight: f64) -> f64 {
    let lowered = text.to_lowercase();
    keywords.iter().filter(|kw| lowered.contains(**kw)).count() as f64 * weight
}

fn score_session_metadata(
    keywords: &[&str],
    session: &crate::domain::conversation::ConversationSession,
) -> f64 {
    score_text(keywords, session.label.as_deref().unwrap_or(""), 3.0)
        + score_text(keywords, session.summary.as_deref().unwrap_or(""), 2.0)
        + score_text(keywords, &session.key, 1.0)
}

fn matches_kind_filter(
    session: &crate::domain::conversation::ConversationSession,
    kind_filter: Option<&str>,
) -> bool {
    if let Some(kind) = kind_filter {
        session.kind.to_string().contains(kind)
    } else {
        true
    }
}

fn score_recipe_keywords(recipe: &RunRecipe, keywords: &[&str]) -> f64 {
    let mut score = 0.0;
    score += score_text(keywords, &recipe.task_family, 3.0);
    score += score_text(keywords, &recipe.sample_request, 2.5);
    score += score_text(keywords, &recipe.summary, 2.0);
    for tool in &recipe.tool_pattern {
        score += score_text(keywords, tool, 0.6);
    }
    score
}

fn event_weight(event: &ConversationEvent) -> f64 {
    match event.event_type {
        EventType::User | EventType::Assistant => 1.6,
        EventType::ToolResult => 1.2,
        EventType::Error => 1.0,
        EventType::ToolCall => 0.6,
        EventType::Interrupted | EventType::System => 0.5,
    }
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max_chars).collect();
    format!("{truncated}...")
}

fn format_event_snippet(event: &ConversationEvent) -> String {
    let actor = match event.event_type {
        EventType::User => "user",
        EventType::Assistant => "assistant",
        EventType::ToolCall => "tool_call",
        EventType::ToolResult => "tool_result",
        EventType::Error => "error",
        EventType::Interrupted => "interrupted",
        EventType::System => "system",
    };
    format!("{actor}: {}", truncate_chars(event.content.trim(), 120))
}

fn transcript_recap(events: &[ConversationEvent], keywords: &[&str]) -> (f64, Option<String>) {
    let mut score = 0.0;
    let mut snippets = Vec::new();

    for event in events {
        let event_score = score_text(keywords, &event.content, event_weight(event));
        if event_score > 0.0 {
            score += event_score;
            if snippets.len() < 2 {
                snippets.push(format_event_snippet(event));
            }
        }
    }

    let recap = if snippets.is_empty() {
        None
    } else {
        Some(snippets.join(" | "))
    };

    (score, recap)
}

fn build_session_document(
    session: &crate::domain::conversation::ConversationSession,
    events: &[ConversationEvent],
) -> String {
    let mut parts = Vec::new();
    if let Some(label) = session.label.as_deref() {
        if !label.trim().is_empty() {
            parts.push(label.trim().to_string());
        }
    }
    if let Some(summary) = session.summary.as_deref() {
        if !summary.trim().is_empty() {
            parts.push(summary.trim().to_string());
        }
    }
    for event in events.iter().take(8) {
        if !event.content.trim().is_empty() {
            parts.push(event.content.trim().to_string());
        }
    }
    parts.join("\n")
}

fn build_recipe_document(recipe: &RunRecipe) -> String {
    let mut parts = Vec::new();
    if !recipe.task_family.trim().is_empty() {
        parts.push(recipe.task_family.trim().to_string());
    }
    if !recipe.sample_request.trim().is_empty() {
        parts.push(recipe.sample_request.trim().to_string());
    }
    if !recipe.summary.trim().is_empty() {
        parts.push(recipe.summary.trim().to_string());
    }
    if !recipe.tool_pattern.is_empty() {
        parts.push(recipe.tool_pattern.join(" "));
    }
    parts.join("\n")
}

fn current_unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn temporal_decay_score(age_secs: u64, half_life_secs: u64) -> f64 {
    if half_life_secs == 0 {
        return 0.0;
    }
    0.5f64.powf(age_secs as f64 / half_life_secs as f64)
}

fn select_with_mmr<T>(
    mut candidates: Vec<RankedCandidate<T>>,
    limit: usize,
    mmr_lambda: f64,
) -> Vec<T> {
    if candidates.is_empty() || limit == 0 {
        return Vec::new();
    }

    let lambda = mmr_lambda.clamp(0.0, 1.0);
    if lambda >= 0.999 || candidates.len() <= 1 {
        candidates.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        candidates.truncate(limit);
        return candidates
            .into_iter()
            .map(|candidate| candidate.item)
            .collect();
    }

    let mut selected = Vec::new();
    let mut selected_embeddings: Vec<Vec<f32>> = Vec::new();

    while !candidates.is_empty() && selected.len() < limit {
        let mut best_idx = 0usize;
        let mut best_score = f64::NEG_INFINITY;

        for (idx, candidate) in candidates.iter().enumerate() {
            let redundancy_penalty = candidate
                .embedding
                .as_deref()
                .map(|embedding| {
                    selected_embeddings
                        .iter()
                        .filter_map(|chosen| cosine_similarity(embedding, chosen))
                        .fold(0.0, f64::max)
                })
                .unwrap_or(0.0);

            let mmr_score = (lambda * candidate.score) - ((1.0 - lambda) * redundancy_penalty);
            if mmr_score > best_score {
                best_score = mmr_score;
                best_idx = idx;
            }
        }

        let chosen = candidates.swap_remove(best_idx);
        if let Some(embedding) = chosen.embedding.clone() {
            selected_embeddings.push(embedding);
        }
        selected.push(chosen.item);
    }

    selected
}

async fn embed_query_or_none(memory: &dyn UnifiedMemoryPort, text: &str) -> Option<Vec<f32>> {
    match memory.embed_query(text).await {
        Ok(embedding) if !embedding.is_empty() => Some(embedding),
        _ => None,
    }
}

async fn embed_document_or_none(memory: &dyn UnifiedMemoryPort, text: &str) -> Option<Vec<f32>> {
    match memory.embed_document(text).await {
        Ok(embedding) if !embedding.is_empty() => Some(embedding),
        _ => None,
    }
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> Option<f64> {
    if left.is_empty() || right.is_empty() || left.len() != right.len() {
        return None;
    }

    let mut dot = 0.0f64;
    let mut left_mag = 0.0f64;
    let mut right_mag = 0.0f64;
    for (l, r) in left.iter().zip(right.iter()) {
        let l = *l as f64;
        let r = *r as f64;
        dot += l * r;
        left_mag += l * l;
        right_mag += r * r;
    }

    if left_mag == 0.0 || right_mag == 0.0 {
        return None;
    }

    Some(dot / (left_mag.sqrt() * right_mag.sqrt()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::conversation::{ConversationEvent, ConversationSession};
    use crate::domain::memory::{
        AgentId, ConsolidationReport, CoreMemoryBlock, HybridSearchResult, MemoryCategory,
        MemoryId, Reflection, SearchResult, SearchSource, SessionId, SkillUpdate, TemporalFact,
        Visibility,
    };
    use crate::domain::run_recipe::RunRecipe;
    use crate::ports::conversation_store::ConversationStorePort;
    use crate::ports::memory::{
        ConsolidationPort, EpisodicMemoryPort, ReflectionPort, SemanticMemoryPort, SkillMemoryPort,
        WorkingMemoryPort,
    };
    use crate::ports::run_recipe_store::InMemoryRunRecipeStore;
    use async_trait::async_trait;
    #[derive(Default)]
    struct StubStore {
        sessions: Vec<ConversationSession>,
        events: std::collections::HashMap<String, Vec<ConversationEvent>>,
    }

    #[async_trait]
    impl ConversationStorePort for StubStore {
        async fn get_session(&self, key: &str) -> Option<ConversationSession> {
            self.sessions.iter().find(|s| s.key == key).cloned()
        }
        async fn list_sessions(&self, _prefix: Option<&str>) -> Vec<ConversationSession> {
            self.sessions.clone()
        }
        async fn upsert_session(&self, _session: &ConversationSession) -> anyhow::Result<()> {
            Ok(())
        }
        async fn delete_session(&self, _key: &str) -> anyhow::Result<bool> {
            Ok(false)
        }
        async fn touch_session(&self, _key: &str) -> anyhow::Result<()> {
            Ok(())
        }
        async fn append_event(
            &self,
            _session_key: &str,
            _event: &ConversationEvent,
        ) -> anyhow::Result<()> {
            Ok(())
        }
        async fn get_events(&self, session_key: &str, _limit: usize) -> Vec<ConversationEvent> {
            self.events.get(session_key).cloned().unwrap_or_default()
        }
        async fn clear_events(&self, _session_key: &str) -> anyhow::Result<()> {
            Ok(())
        }
        async fn update_label(&self, _key: &str, _label: &str) -> anyhow::Result<()> {
            Ok(())
        }
        async fn update_goal(&self, _key: &str, _goal: &str) -> anyhow::Result<()> {
            Ok(())
        }
        async fn increment_message_count(&self, _key: &str) -> anyhow::Result<()> {
            Ok(())
        }
        async fn add_token_usage(
            &self,
            _key: &str,
            _input: i64,
            _output: i64,
        ) -> anyhow::Result<()> {
            Ok(())
        }
        async fn get_summary(&self, _key: &str) -> Option<String> {
            None
        }
        async fn set_summary(&self, _key: &str, _summary: &str) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn session_search_prefers_summary_and_transcript_matches() {
        let store = StubStore {
            sessions: vec![ConversationSession {
                key: "channel:1".into(),
                kind: ConversationKind::Channel,
                label: Some("Server work".into()),
                summary: Some("Discussed deploy on VPS".into()),
                current_goal: None,
                created_at: 1,
                last_active: 10,
                message_count: 4,
                input_tokens: 0,
                output_tokens: 0,
            }],
            events: std::collections::HashMap::from([(
                "channel:1".into(),
                vec![ConversationEvent {
                    event_type: EventType::Assistant,
                    actor: "assistant".into(),
                    content: "We deployed to VPS through Docker".into(),
                    tool_name: None,
                    run_id: None,
                    input_tokens: None,
                    output_tokens: None,
                    timestamp: 1,
                }],
            )]),
        };

        let hits = search_sessions(&StubMemory::default(), &store, "deploy vps", None, 5).await;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session_key, "channel:1");
        assert!(hits[0]
            .recap
            .as_deref()
            .unwrap_or_default()
            .contains("assistant:"));
    }

    #[tokio::test]
    async fn session_search_respects_kind_filter() {
        let store = StubStore {
            sessions: vec![
                ConversationSession {
                    key: "web:1".into(),
                    kind: ConversationKind::Web,
                    label: Some("Web deploy".into()),
                    summary: Some("Deploy".into()),
                    current_goal: None,
                    created_at: 1,
                    last_active: 10,
                    message_count: 1,
                    input_tokens: 0,
                    output_tokens: 0,
                },
                ConversationSession {
                    key: "channel:1".into(),
                    kind: ConversationKind::Channel,
                    label: Some("Channel deploy".into()),
                    summary: Some("Deploy".into()),
                    current_goal: None,
                    created_at: 1,
                    last_active: 11,
                    message_count: 1,
                    input_tokens: 0,
                    output_tokens: 0,
                },
            ],
            events: Default::default(),
        };

        let hits =
            search_sessions(&StubMemory::default(), &store, "deploy", Some("channel"), 5).await;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, ConversationKind::Channel);
    }

    #[derive(Default)]
    struct StubMemory {
        hybrid: HybridSearchResult,
    }

    #[async_trait]
    impl WorkingMemoryPort for StubMemory {
        async fn get_core_blocks(&self, _: &AgentId) -> Result<Vec<CoreMemoryBlock>, MemoryError> {
            Ok(vec![])
        }
        async fn update_core_block(
            &self,
            _: &AgentId,
            _: &str,
            _: String,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn append_core_block(
            &self,
            _: &AgentId,
            _: &str,
            _: &str,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
    }

    #[async_trait]
    impl EpisodicMemoryPort for StubMemory {
        async fn store_episode(&self, _: MemoryEntry) -> Result<MemoryId, MemoryError> {
            Ok(String::new())
        }
        async fn get_recent(&self, _: &AgentId, _: usize) -> Result<Vec<MemoryEntry>, MemoryError> {
            Ok(vec![])
        }
        async fn get_session(&self, _: &SessionId) -> Result<Vec<MemoryEntry>, MemoryError> {
            Ok(vec![])
        }
        async fn search_episodes(&self, _: &MemoryQuery) -> Result<Vec<SearchResult>, MemoryError> {
            Ok(vec![])
        }
    }

    #[async_trait]
    impl SemanticMemoryPort for StubMemory {
        async fn upsert_entity(&self, _: Entity) -> Result<MemoryId, MemoryError> {
            Ok(String::new())
        }
        async fn find_entity(&self, _: &str) -> Result<Option<Entity>, MemoryError> {
            Ok(None)
        }
        async fn add_fact(&self, _: TemporalFact) -> Result<MemoryId, MemoryError> {
            Ok(String::new())
        }
        async fn invalidate_fact(&self, _: &MemoryId) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn get_current_facts(&self, _: &MemoryId) -> Result<Vec<TemporalFact>, MemoryError> {
            Ok(vec![])
        }
        async fn traverse(
            &self,
            _: &MemoryId,
            _: usize,
        ) -> Result<Vec<(Entity, TemporalFact)>, MemoryError> {
            Ok(vec![])
        }
        async fn search_entities(&self, _: &MemoryQuery) -> Result<Vec<Entity>, MemoryError> {
            Ok(vec![])
        }
    }

    #[async_trait]
    impl SkillMemoryPort for StubMemory {
        async fn store_skill(&self, _: Skill) -> Result<MemoryId, MemoryError> {
            Ok(String::new())
        }
        async fn find_skills(&self, _: &MemoryQuery) -> Result<Vec<Skill>, MemoryError> {
            Ok(vec![])
        }
        async fn update_skill(
            &self,
            _: &MemoryId,
            _: SkillUpdate,
            _: &AgentId,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn get_skill(&self, _: &str, _: &AgentId) -> Result<Option<Skill>, MemoryError> {
            Ok(None)
        }
    }

    #[async_trait]
    impl ReflectionPort for StubMemory {
        async fn store_reflection(&self, _: Reflection) -> Result<MemoryId, MemoryError> {
            Ok(String::new())
        }
        async fn get_relevant_reflections(
            &self,
            _: &MemoryQuery,
        ) -> Result<Vec<Reflection>, MemoryError> {
            Ok(vec![])
        }
        async fn get_failure_patterns(
            &self,
            _: &AgentId,
            _: usize,
        ) -> Result<Vec<Reflection>, MemoryError> {
            Ok(vec![])
        }
    }

    #[async_trait]
    impl ConsolidationPort for StubMemory {
        async fn run_consolidation(&self, _: &AgentId) -> Result<ConsolidationReport, MemoryError> {
            Ok(ConsolidationReport::default())
        }
        async fn recalculate_importance(&self, _: &AgentId) -> Result<u32, MemoryError> {
            Ok(0)
        }
        async fn gc_low_importance(&self, _: f32, _: u32) -> Result<u32, MemoryError> {
            Ok(0)
        }
    }

    #[async_trait]
    impl UnifiedMemoryPort for StubMemory {
        async fn hybrid_search(&self, _: &MemoryQuery) -> Result<HybridSearchResult, MemoryError> {
            Ok(self.hybrid.clone())
        }
        async fn embed(&self, text: &str) -> Result<Vec<f32>, MemoryError> {
            Ok(test_embedding(text))
        }
        async fn store(
            &self,
            _: &str,
            _: &str,
            _: &MemoryCategory,
            _: Option<&str>,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn recall(
            &self,
            _: &str,
            _: usize,
            _: Option<&str>,
        ) -> Result<Vec<MemoryEntry>, MemoryError> {
            Ok(vec![])
        }
        async fn consolidate_turn(&self, _: &str, _: &str) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn forget(&self, _: &str, _: &AgentId) -> Result<bool, MemoryError> {
            Ok(false)
        }
        async fn get(&self, _: &str, _: &AgentId) -> Result<Option<MemoryEntry>, MemoryError> {
            Ok(None)
        }
        async fn list(
            &self,
            _: Option<&MemoryCategory>,
            _: Option<&str>,
            _: usize,
        ) -> Result<Vec<MemoryEntry>, MemoryError> {
            Ok(vec![])
        }
        fn should_skip_autosave(&self, _: &str) -> bool {
            false
        }
        async fn count(&self) -> Result<usize, MemoryError> {
            Ok(0)
        }
        fn name(&self) -> &str {
            "stub"
        }
        async fn health_check(&self) -> bool {
            true
        }
        async fn promote_visibility(
            &self,
            _: &MemoryId,
            _: &Visibility,
            _: &[AgentId],
            _: &AgentId,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn hybrid_turn_search_filters_noise_and_scopes_session() {
        let memory = StubMemory {
            hybrid: HybridSearchResult {
                episodes: vec![
                    SearchResult {
                        entry: MemoryEntry {
                            id: "1".into(),
                            key: "assistant_resp".into(),
                            content: "noise".into(),
                            category: MemoryCategory::Conversation,
                            timestamp: String::new(),
                            session_id: Some("s1".into()),
                            score: None,
                        },
                        score: 0.9,
                        source: SearchSource::Hybrid,
                    },
                    SearchResult {
                        entry: MemoryEntry {
                            id: "2".into(),
                            key: "weather".into(),
                            content: "Berlin weather context".into(),
                            category: MemoryCategory::Conversation,
                            timestamp: String::new(),
                            session_id: Some("s1".into()),
                            score: None,
                        },
                        score: 0.8,
                        source: SearchSource::Hybrid,
                    },
                    SearchResult {
                        entry: MemoryEntry {
                            id: "3".into(),
                            key: "other".into(),
                            content: "wrong session".into(),
                            category: MemoryCategory::Conversation,
                            timestamp: String::new(),
                            session_id: Some("s2".into()),
                            score: None,
                        },
                        score: 0.95,
                        source: SearchSource::Hybrid,
                    },
                ],
                entities: vec![Entity {
                    id: "e1".into(),
                    name: "Berlin".into(),
                    entity_type: "city".into(),
                    properties: serde_json::json!({}),
                    summary: Some("German city".into()),
                    created_by: "agent".into(),
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                }],
                facts: vec![],
                skills: vec![Skill {
                    id: "sk1".into(),
                    name: "weather".into(),
                    description: String::new(),
                    content: "Check a weather source".into(),
                    task_family: Some("weather".into()),
                    lineage_task_families: vec!["weather".into()],
                    tool_pattern: vec!["web_search".into()],
                    tags: vec![],
                    success_count: 1,
                    fail_count: 0,
                    version: 1,
                    origin: crate::domain::memory::SkillOrigin::Learned,
                    status: crate::domain::memory::SkillStatus::Active,
                    created_by: "agent".into(),
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                }],
                reflections: vec![],
            },
        };

        let hits = search_turn_hybrid(
            &memory,
            "weather",
            "agent",
            Some("s1"),
            &HybridTurnSearchOptions {
                recall_max_entries: 3,
                recall_min_relevance: 0.4,
                skills_max_count: 2,
                skills_total_max_chars: 200,
                entities_max_count: 2,
                entities_total_max_chars: 200,
                query_limit: 8,
            },
        )
        .await
        .unwrap();

        assert_eq!(hits.recalled_entries.len(), 1);
        assert_eq!(hits.recalled_entries[0].key, "weather");
        assert_eq!(hits.skills.len(), 1);
        assert_eq!(hits.entities.len(), 1);
    }

    #[tokio::test]
    async fn hybrid_turn_search_prefers_manual_active_skills_and_skips_candidates() {
        let memory = StubMemory {
            hybrid: HybridSearchResult {
                episodes: vec![],
                entities: vec![],
                facts: vec![],
                skills: vec![
                    Skill {
                        id: "sk-learned".into(),
                        name: "learned-weather".into(),
                        description: String::new(),
                        content: "Use a weather source".into(),
                        task_family: Some("weather".into()),
                        lineage_task_families: vec!["weather".into()],
                        tool_pattern: vec!["web_search".into()],
                        tags: vec![],
                        success_count: 50,
                        fail_count: 1,
                        version: 2,
                        origin: crate::domain::memory::SkillOrigin::Learned,
                        status: crate::domain::memory::SkillStatus::Active,
                        created_by: "agent".into(),
                        created_at: chrono::Utc::now(),
                        updated_at: chrono::Utc::now(),
                    },
                    Skill {
                        id: "sk-manual".into(),
                        name: "manual-weather".into(),
                        description: String::new(),
                        content: "Prefer official sources first".into(),
                        task_family: Some("weather".into()),
                        lineage_task_families: vec!["weather".into()],
                        tool_pattern: vec!["web_search".into()],
                        tags: vec![],
                        success_count: 1,
                        fail_count: 0,
                        version: 1,
                        origin: crate::domain::memory::SkillOrigin::Manual,
                        status: crate::domain::memory::SkillStatus::Active,
                        created_by: "agent".into(),
                        created_at: chrono::Utc::now(),
                        updated_at: chrono::Utc::now(),
                    },
                    Skill {
                        id: "sk-candidate".into(),
                        name: "candidate-weather".into(),
                        description: String::new(),
                        content: "Unproven draft".into(),
                        task_family: Some("weather".into()),
                        lineage_task_families: vec!["weather".into()],
                        tool_pattern: vec!["web_search".into()],
                        tags: vec![],
                        success_count: 99,
                        fail_count: 0,
                        version: 1,
                        origin: crate::domain::memory::SkillOrigin::Learned,
                        status: crate::domain::memory::SkillStatus::Candidate,
                        created_by: "agent".into(),
                        created_at: chrono::Utc::now(),
                        updated_at: chrono::Utc::now(),
                    },
                ],
                reflections: vec![],
            },
        };

        let hits = search_turn_hybrid(
            &memory,
            "weather",
            "agent",
            None,
            &HybridTurnSearchOptions {
                recall_max_entries: 3,
                recall_min_relevance: 0.4,
                skills_max_count: 3,
                skills_total_max_chars: 500,
                entities_max_count: 0,
                entities_total_max_chars: 0,
                query_limit: 8,
            },
        )
        .await
        .unwrap();

        assert_eq!(hits.skills.len(), 2);
        assert_eq!(hits.skills[0].name, "manual-weather");
        assert_eq!(hits.skills[1].name, "learned-weather");
        assert!(hits
            .skills
            .iter()
            .all(|skill| matches!(skill.status, crate::domain::memory::SkillStatus::Active)));
    }

    #[tokio::test]
    async fn session_search_uses_semantic_similarity_for_paraphrases() {
        let store = StubStore {
            sessions: vec![
                ConversationSession {
                    key: "web:deploy".into(),
                    kind: ConversationKind::Web,
                    label: Some("Release rollout".into()),
                    summary: Some("Deploy release to production".into()),
                    current_goal: None,
                    created_at: 1,
                    last_active: 20,
                    message_count: 3,
                    input_tokens: 0,
                    output_tokens: 0,
                },
                ConversationSession {
                    key: "web:weather".into(),
                    kind: ConversationKind::Web,
                    label: Some("Weather lookup".into()),
                    summary: Some("Checked Berlin weather".into()),
                    current_goal: None,
                    created_at: 1,
                    last_active: 19,
                    message_count: 2,
                    input_tokens: 0,
                    output_tokens: 0,
                },
            ],
            events: Default::default(),
        };

        let hits =
            search_sessions(&StubMemory::default(), &store, "ship it to prod", None, 5).await;
        assert_eq!(
            hits.first().map(|hit| hit.session_key.as_str()),
            Some("web:deploy")
        );
    }

    #[tokio::test]
    async fn run_recipe_search_uses_semantic_similarity() {
        let store = InMemoryRunRecipeStore::new();
        store
            .upsert(RunRecipe {
                agent_id: "agent".into(),
                task_family: "deploy".into(),
                lineage_task_families: vec!["deploy".into()],
                sample_request: "deploy the latest release to production".into(),
                summary: "Check staging first, then ship the release".into(),
                tool_pattern: vec!["shell".into(), "git".into()],
                success_count: 3,
                updated_at: 10,
            })
            .unwrap();
        store
            .upsert(RunRecipe {
                agent_id: "agent".into(),
                task_family: "weather".into(),
                lineage_task_families: vec!["weather".into()],
                sample_request: "check Berlin weather".into(),
                summary: "Open the forecast".into(),
                tool_pattern: vec!["web_fetch".into()],
                success_count: 2,
                updated_at: 9,
            })
            .unwrap();

        let hits = search_run_recipes(
            &StubMemory::default(),
            &store,
            "agent",
            "ship it to prod",
            5,
        )
        .await;

        assert_eq!(
            hits.first().map(|hit| hit.task_family.as_str()),
            Some("deploy")
        );
    }

    #[tokio::test]
    async fn session_search_options_can_prefer_recent_sessions() {
        let now = current_unix_seconds();
        let store = StubStore {
            sessions: vec![
                ConversationSession {
                    key: "web:old".into(),
                    kind: ConversationKind::Web,
                    label: Some("Deploy run".into()),
                    summary: Some("Deploy release".into()),
                    current_goal: None,
                    created_at: 1,
                    last_active: now.saturating_sub(60 * 60 * 24 * 30),
                    message_count: 3,
                    input_tokens: 0,
                    output_tokens: 0,
                },
                ConversationSession {
                    key: "web:new".into(),
                    kind: ConversationKind::Web,
                    label: Some("Deploy run".into()),
                    summary: Some("Deploy release".into()),
                    current_goal: None,
                    created_at: 1,
                    last_active: now.saturating_sub(60 * 10),
                    message_count: 3,
                    input_tokens: 0,
                    output_tokens: 0,
                },
            ],
            events: Default::default(),
        };

        let hits = search_sessions_with_options(
            &StubMemory::default(),
            &store,
            "deploy release",
            &SessionSearchOptions {
                limit: 2,
                ..SessionSearchOptions::default()
            },
        )
        .await;

        assert_eq!(
            hits.first().map(|hit| hit.session_key.as_str()),
            Some("web:new")
        );
    }

    #[test]
    fn mmr_selection_can_diversify_near_duplicates() {
        let hits = select_with_mmr(
            vec![
                RankedCandidate {
                    score: 10.0,
                    embedding: Some(vec![1.0, 0.0]),
                    item: "deploy-a",
                },
                RankedCandidate {
                    score: 9.8,
                    embedding: Some(vec![1.0, 0.0]),
                    item: "deploy-b",
                },
                RankedCandidate {
                    score: 8.9,
                    embedding: Some(vec![0.0, 1.0]),
                    item: "restart",
                },
            ],
            2,
            0.4,
        );

        assert_eq!(hits, vec!["deploy-a", "restart"]);
    }

    fn test_embedding(text: &str) -> Vec<f32> {
        let lowered = text.to_lowercase();
        let mut vec = vec![0.0f32; 4];

        for token in lowered.split(|c: char| !c.is_alphanumeric()) {
            match token {
                "deploy" | "release" | "rollout" | "ship" => vec[0] += 1.0,
                "prod" | "production" => vec[1] += 1.0,
                "weather" | "forecast" => vec[2] += 1.0,
                "berlin" | "city" => vec[3] += 1.0,
                _ => {}
            }
        }

        vec
    }
}
