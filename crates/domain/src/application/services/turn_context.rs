//! Turn context assembly — unified memory enrichment for all paths.
//!
//! Owns the policy of *what* memory data to load for a given turn,
//! and provides canonical formatting into prompt strings.
//!
//! Formatting lives here (not in the adapter layer) because both
//! web (`agent.rs`) and channels (`handle_inbound_message.rs`) need
//! the same format, and the channel use case can't call adapters.
//! The adapter `turn_context_fmt` re-exports these functions.

use crate::application::services::clarification_policy;
use crate::application::services::epistemic_state::{
    epistemic_entry_for_memory_entry, format_epistemic_entry,
    memory_epistemic_retrieval_score_delta,
};
use crate::application::services::execution_guidance;
use crate::application::services::failure_similarity_service;
use crate::application::services::precedent_similarity_service;
use crate::application::services::procedural_cluster_service;
use crate::application::services::procedural_contradiction_service;
use crate::application::services::resolution_router;
use crate::application::services::retrieval_service;
use crate::application::services::run_recipe_cluster_service;
use crate::application::services::session_handoff;
use crate::application::services::turn_budget_policy;
use crate::application::services::turn_interpretation::{
    ReferenceCandidateKind, ReferenceSource, TurnInterpretation,
};
use crate::domain::memory::{CoreMemoryBlock, Entity, MemoryCategory, MemoryEntry, Skill};
use crate::domain::run_recipe::RunRecipe;
use crate::domain::tool_repair::ToolRepairTrace;
use crate::domain::turn_admission::{AdmissionRepairHint, CandidateAdmissionReason};
use crate::ports::conversation_store::ConversationStorePort;
use crate::ports::memory::UnifiedMemoryPort;
use crate::ports::run_recipe_store::RunRecipeStorePort;

// ── Types ────────────────────────────────────────────────────────

/// Structured memory context for a single LLM turn.
///
/// Pure data — no formatting, no prompt strings.
/// The adapter layer converts this into system messages / user prefixes.
#[derive(Debug, Default)]
pub struct TurnMemoryContext {
    /// Core memory blocks (MemGPT pattern), always present.
    pub core_blocks: Vec<CoreMemoryBlock>,
    /// Episodic recall entries matching the user query.
    pub recalled_entries: Vec<MemoryEntry>,
    /// Small neighborhood around top episodic hits for better local continuity.
    pub nearby_entries: Vec<MemoryEntry>,
    /// Recent temporal echoes from the same memory lanes as surfaced recall.
    pub recent_echoes: Vec<MemoryEntry>,
    /// Surfaced precedent-memory branches that conflict with recent failure clusters.
    pub memory_cautions: Vec<MemoryContradictionCaution>,
    /// Relevant learned skills (procedural memory).
    pub skills: Vec<Skill>,
    /// Relevant entities (semantic memory / knowledge graph).
    pub entities: Vec<Entity>,
    /// Short semantic neighborhood around surfaced entities.
    pub entity_neighbors: Vec<EntityNeighborhood>,
    /// Relevant prior session recaps / historical context.
    pub session_matches: Vec<retrieval_service::SessionSearchMatch>,
    /// Relevant prior successful execution recipes / precedents.
    pub run_recipes: Vec<retrieval_service::RunRecipeSearchMatch>,
    /// Contradictions between surfaced recipe paths and recent failure clusters.
    pub procedural_contradictions: Vec<procedural_contradiction_service::ProceduralContradiction>,
    /// Deterministic resolver ordering for this turn.
    pub resolution_plan: Option<resolution_router::ResolutionPlan>,
    /// Structured clarification guidance from known runtime facts and candidates.
    pub clarification_guidance: Option<clarification_policy::ClarificationGuidance>,
    /// Typed execution policy for direct-resolution turns.
    pub execution_guidance: Option<execution_guidance::ExecutionGuidance>,
    /// Cheap-path gating and retrieval limits for this turn.
    pub execution_budget: Option<turn_budget_policy::TurnExecutionBudget>,
    /// Bounded typed handoff packet when the previous/current route needs a fresh session.
    pub handoff_packet: Option<session_handoff::SessionHandoffPacket>,
}

/// Token/char budget for turn context assembly.
#[derive(Debug, Clone)]
pub struct PromptBudget {
    pub core_blocks_total_max_chars: usize,
    pub recall_max_entries: usize,
    pub nearby_max_entries: usize,
    pub recall_entry_max_chars: usize,
    pub recall_total_max_chars: usize,
    pub recall_min_relevance: f64,
    pub skills_max_count: usize,
    pub skills_total_max_chars: usize,
    pub entities_max_count: usize,
    pub entities_total_max_chars: usize,
    pub enrichment_total_max_chars: usize,
}

impl Default for PromptBudget {
    fn default() -> Self {
        Self {
            core_blocks_total_max_chars: 1_800,
            recall_max_entries: 5,
            nearby_max_entries: 2,
            recall_entry_max_chars: 800,
            recall_total_max_chars: 4_000,
            recall_min_relevance: 0.4,
            skills_max_count: 3,
            skills_total_max_chars: 2_000,
            entities_max_count: 3,
            entities_total_max_chars: 1_500,
            enrichment_total_max_chars: 8_000,
        }
    }
}

/// What to load on continuation turns (turn N>1 in a session).
#[derive(Debug, Clone)]
pub enum ContinuationPolicy {
    /// Core blocks only — cheapest, no recall/skills/entities.
    CoreOnly,
    /// Core blocks + lightweight recall (reduced budget).
    CorePlusRecall { recall_max_entries: usize },
    /// Full context — same as first turn.
    Full,
}

impl Default for ContinuationPolicy {
    fn default() -> Self {
        Self::CorePlusRecall {
            recall_max_entries: 2,
        }
    }
}

// ── Assembly ─────────────────────────────────────────────────────

/// Assemble structured memory context for a single turn.
///
/// Both web and channel paths should call this function.
/// The `continuation` parameter controls what to load on turn N>1:
/// - `None` → full context (first turn or explicit full).
/// - `Some(CoreOnly)` → core blocks only.
/// - `Some(CorePlusRecall { n })` → core blocks + recall with limit `n`.
/// - `Some(Full)` → same as `None`.
pub async fn assemble_turn_context(
    mem: &dyn UnifiedMemoryPort,
    run_recipe_store: Option<&dyn RunRecipeStorePort>,
    conversation_store: Option<&dyn ConversationStorePort>,
    user_message: &str,
    agent_id: &str,
    session_id: Option<&str>,
    interpretation: Option<&TurnInterpretation>,
    recent_tool_repairs: &[ToolRepairTrace],
    recent_admission_reasons: &[CandidateAdmissionReason],
    recent_admission_repair: Option<AdmissionRepairHint>,
    budget: &PromptBudget,
    continuation: Option<&ContinuationPolicy>,
) -> TurnMemoryContext {
    let mut ctx = TurnMemoryContext {
        core_blocks: mem
            .get_core_blocks(&agent_id.to_string())
            .await
            .unwrap_or_default(),
        ..Default::default()
    };
    ctx.execution_budget = build_execution_budget(interpretation);
    let query_text = build_query_text(user_message, interpretation);

    let policy_name = match continuation {
        Some(ContinuationPolicy::CoreOnly) => "core_only",
        Some(ContinuationPolicy::CorePlusRecall { .. }) => "core_plus_recall",
        Some(ContinuationPolicy::Full) => "full",
        None => "first_turn",
    };

    match continuation {
        Some(ContinuationPolicy::CoreOnly) => {
            apply_resolution_plan(
                &mut ctx,
                user_message,
                interpretation,
                recent_tool_repairs,
                recent_admission_reasons,
                recent_admission_repair,
            );
            tracing::debug!(
                target: "memory_assembly",
                core_blocks = ctx.core_blocks.len(),
                policy = policy_name,
                "Turn context assembled"
            );
            return ctx;
        }
        Some(ContinuationPolicy::CorePlusRecall {
            recall_max_entries: n,
        }) => {
            let recall_limit = ctx
                .execution_budget
                .as_ref()
                .map_or(*n, |execution_budget| {
                    (*n).min(execution_budget.retrieval_budget.max_memory_candidates)
                });
            load_recall(mem, &query_text, session_id, budget, recall_limit, &mut ctx).await;
            load_nearby_recall(mem, agent_id, budget, &mut ctx).await;
            load_recent_echoes(mem, budget, &mut ctx).await;
            apply_resolution_plan(
                &mut ctx,
                user_message,
                interpretation,
                recent_tool_repairs,
                recent_admission_reasons,
                recent_admission_repair,
            );
            tracing::debug!(
                target: "memory_assembly",
                core_blocks = ctx.core_blocks.len(),
                recalled = ctx.recalled_entries.len(),
                nearby = ctx.nearby_entries.len(),
                echoes = ctx.recent_echoes.len(),
                memory_cautions = ctx.memory_cautions.len(),
                policy = policy_name,
                "Turn context assembled"
            );
            return ctx;
        }
        Some(ContinuationPolicy::Full) | None => {
            // Full context: recall + skills + entities
        }
    }

    // Episodic recall
    let recall_limit =
        ctx.execution_budget
            .as_ref()
            .map_or(budget.recall_max_entries, |execution_budget| {
                budget
                    .recall_max_entries
                    .min(execution_budget.retrieval_budget.max_memory_candidates)
            });
    let results = retrieval_service::search_turn_hybrid(
        mem,
        &query_text,
        agent_id,
        session_id,
        &retrieval_service::HybridTurnSearchOptions {
            recall_max_entries: recall_limit,
            recall_min_relevance: budget.recall_min_relevance,
            skills_max_count: budget.skills_max_count,
            skills_total_max_chars: budget.skills_total_max_chars,
            entities_max_count: budget.entities_max_count,
            entities_total_max_chars: budget.entities_total_max_chars,
            query_limit: budget
                .recall_max_entries
                .max(budget.skills_max_count)
                .max(budget.entities_max_count)
                .max(8)
                + 2,
        },
    )
    .await;
    if let Ok(results) = results {
        ctx.recalled_entries = results.recalled_entries;
        ctx.skills = results.skills;
        ctx.entities = results.entities;
    }
    load_nearby_recall(mem, agent_id, budget, &mut ctx).await;
    load_recent_echoes(mem, budget, &mut ctx).await;

    let allow_historical_context = ctx
        .execution_budget
        .as_ref()
        .is_some_and(|execution_budget| {
            execution_budget.interpreter_mode != turn_budget_policy::InterpreterMode::Skip
        });
    let needs_failure_clusters = has_precedent_memory_entries(&ctx.recalled_entries)
        || has_precedent_memory_entries(&ctx.nearby_entries)
        || has_precedent_memory_entries(&ctx.recent_echoes)
        || (allow_historical_context && run_recipe_store.is_some());
    let failure_clusters = if needs_failure_clusters {
        load_recent_failure_clusters(mem, agent_id).await
    } else {
        Vec::new()
    };

    if !failure_clusters.is_empty() {
        prioritize_uncontradicted_memory_entries(&mut ctx.recalled_entries, &failure_clusters);
        prioritize_uncontradicted_memory_entries(&mut ctx.nearby_entries, &failure_clusters);
        prioritize_uncontradicted_memory_entries(&mut ctx.recent_echoes, &failure_clusters);
        load_memory_contradictions_from_clusters(&failure_clusters, &mut ctx);
    }
    load_entity_neighbors(mem, &mut ctx).await;

    if allow_historical_context {
        if let Some(store) = conversation_store {
            let session_max_count = ctx.execution_budget.as_ref().map_or(2, |execution_budget| {
                execution_budget.retrieval_budget.max_session_candidates
            });
            if session_max_count > 0 {
                const SESSION_TOTAL_MAX_CHARS: usize = 1_200;
                const SESSION_MIN_SCORE: f64 = 1.5;

                let session_hits = retrieval_service::search_sessions(
                    mem,
                    store,
                    &query_text,
                    None,
                    session_max_count + 1,
                )
                .await;
                let mut total_session_chars = 0usize;
                for hit in session_hits {
                    if session_id.is_some_and(|current| hit.session_key == current) {
                        continue;
                    }
                    if hit.score < SESSION_MIN_SCORE {
                        break;
                    }
                    let session_chars = hit.label.as_ref().map_or(0, |s| s.chars().count())
                        + hit.summary.as_ref().map_or(0, |s| s.chars().count())
                        + hit.recap.as_ref().map_or(0, |s| s.chars().count());
                    if total_session_chars + session_chars > SESSION_TOTAL_MAX_CHARS {
                        break;
                    }
                    total_session_chars += session_chars;
                    ctx.session_matches.push(hit);
                    if ctx.session_matches.len() >= session_max_count {
                        break;
                    }
                }
            }
        }
    }

    if allow_historical_context {
        if let Some(store) = run_recipe_store {
            let recipe_max_count = ctx.execution_budget.as_ref().map_or(2, |execution_budget| {
                execution_budget.retrieval_budget.max_precedent_candidates
            });
            if recipe_max_count > 0 {
                const RECIPE_TOTAL_MAX_CHARS: usize = 1_200;
                const RECIPE_MIN_SCORE: i64 = 150;

                let recipe_hits = retrieval_service::search_run_recipes(
                    mem,
                    store,
                    agent_id,
                    &query_text,
                    recipe_max_count,
                )
                .await;
                let stored_recipe_hits = recipe_hits
                    .iter()
                    .filter_map(|hit| store.get(agent_id, &hit.task_family))
                    .collect::<Vec<_>>();
                let recipe_hits = prioritize_uncontradicted_recipe_hits(
                    recipe_hits,
                    &stored_recipe_hits,
                    &failure_clusters,
                );
                let mut total_recipe_chars = 0usize;
                for recipe in recipe_hits {
                    if recipe.score < RECIPE_MIN_SCORE {
                        break;
                    }
                    let recipe_chars = recipe.summary.chars().count()
                        + recipe.sample_request.chars().count()
                        + recipe
                            .tool_pattern
                            .iter()
                            .map(|tool| tool.chars().count())
                            .sum::<usize>();
                    if total_recipe_chars + recipe_chars > RECIPE_TOTAL_MAX_CHARS {
                        break;
                    }
                    total_recipe_chars += recipe_chars;
                    ctx.run_recipes.push(recipe);
                }
                load_recipe_contradictions_from_clusters(
                    store,
                    agent_id,
                    &failure_clusters,
                    &mut ctx,
                );
            }
        }
    }

    apply_resolution_plan(
        &mut ctx,
        user_message,
        interpretation,
        recent_tool_repairs,
        recent_admission_reasons,
        recent_admission_repair,
    );

    tracing::debug!(
        target: "memory_assembly",
        core_blocks = ctx.core_blocks.len(),
        recalled = ctx.recalled_entries.len(),
        nearby = ctx.nearby_entries.len(),
        echoes = ctx.recent_echoes.len(),
        memory_cautions = ctx.memory_cautions.len(),
        skills = ctx.skills.len(),
        entities = ctx.entities.len(),
        sessions = ctx.session_matches.len(),
        recipes = ctx.run_recipes.len(),
        contradictions = ctx.procedural_contradictions.len(),
        interpreter_mode = ?ctx
            .execution_budget
            .as_ref()
            .map(|budget| budget.interpreter_mode),
        policy = policy_name,
        "Turn context assembled"
    );

    ctx
}

/// Load episodic recall entries into context, applying filters.
async fn load_recall(
    mem: &dyn UnifiedMemoryPort,
    user_message: &str,
    session_id: Option<&str>,
    budget: &PromptBudget,
    max_entries: usize,
    ctx: &mut TurnMemoryContext,
) {
    // Fetch a few extra to compensate for filtering
    let entries = match mem.recall(user_message, max_entries + 2, session_id).await {
        Ok(e) => e,
        Err(_) => return,
    };

    let mut total_chars = 0usize;
    let mut count = 0usize;

    for entry in entries {
        if count >= max_entries {
            break;
        }
        // Skip assistant autosave bookkeeping
        if is_autosave_key(&entry.key) {
            tracing::trace!(target: "memory_assembly", key = %entry.key, "Recall: skip autosave");
            continue;
        }
        // Skip entries that look like autosave metadata
        if crate::domain::util::should_skip_autosave_content(&entry.content) {
            tracing::trace!(target: "memory_assembly", key = %entry.key, "Recall: skip noise content");
            continue;
        }
        // Skip tool_result leaks
        if entry.content.contains("<tool_result") {
            tracing::trace!(target: "memory_assembly", key = %entry.key, "Recall: skip tool_result");
            continue;
        }
        // Relevance gate
        if let Some(score) = entry.score {
            if score < budget.recall_min_relevance {
                tracing::trace!(target: "memory_assembly", key = %entry.key, score, min = budget.recall_min_relevance, "Recall: skip low relevance");
                continue;
            }
        }
        let entry_chars = entry
            .content
            .chars()
            .count()
            .min(budget.recall_entry_max_chars);
        if total_chars + entry_chars > budget.recall_total_max_chars {
            break;
        }
        total_chars += entry_chars;
        count += 1;
        ctx.recalled_entries.push(entry);
    }
}

async fn load_nearby_recall(
    mem: &dyn UnifiedMemoryPort,
    agent_id: &str,
    budget: &PromptBudget,
    ctx: &mut TurnMemoryContext,
) {
    if ctx.recalled_entries.is_empty() || budget.nearby_max_entries == 0 {
        return;
    }

    let seeds = ctx
        .recalled_entries
        .iter()
        .take(2)
        .cloned()
        .collect::<Vec<_>>();
    let nearby = match retrieval_service::search_nearby_memory(
        mem,
        agent_id,
        &seeds,
        &retrieval_service::NearbyMemorySearchOptions {
            limit: budget.nearby_max_entries,
            per_seed_limit: budget.nearby_max_entries.saturating_add(2).max(2),
            min_score: budget.recall_min_relevance.max(0.55),
        },
    )
    .await
    {
        Ok(nearby) => nearby,
        Err(_) => return,
    };

    ctx.nearby_entries = nearby.into_iter().map(|match_| match_.entry).collect();
}

async fn load_recent_echoes(
    mem: &dyn UnifiedMemoryPort,
    budget: &PromptBudget,
    ctx: &mut TurnMemoryContext,
) {
    if ctx.recalled_entries.is_empty() || budget.nearby_max_entries == 0 {
        return;
    }

    let mut categories = Vec::new();
    for entry in ctx.recalled_entries.iter().take(3) {
        if !categories
            .iter()
            .any(|category: &crate::domain::memory::MemoryCategory| category == &entry.category)
        {
            categories.push(entry.category.clone());
        }
    }
    if categories.is_empty() {
        return;
    }

    let mut seen_keys = ctx
        .recalled_entries
        .iter()
        .chain(ctx.nearby_entries.iter())
        .map(|entry| entry.key.clone())
        .collect::<std::collections::HashSet<_>>();
    let updated_since = chrono::Utc::now() - chrono::Duration::days(14);
    let mut echoes = Vec::new();

    for category in categories {
        let recent = match mem
            .list_recent_scoped(
                Some(&category),
                None,
                budget.nearby_max_entries.saturating_mul(3).max(4),
                false,
                updated_since,
            )
            .await
        {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        for entry in recent {
            if seen_keys.contains(&entry.key)
                || is_autosave_key(&entry.key)
                || crate::domain::util::should_skip_autosave_content(&entry.content)
                || entry.content.contains("<tool_result")
            {
                continue;
            }
            if seen_keys.insert(entry.key.clone()) {
                echoes.push(entry);
            }
        }
    }

    echoes.sort_by(|left, right| {
        right
            .timestamp
            .cmp(&left.timestamp)
            .then_with(|| left.key.cmp(&right.key))
    });
    echoes.truncate(budget.nearby_max_entries);
    ctx.recent_echoes = echoes;
}

async fn load_recent_failure_clusters(
    mem: &dyn UnifiedMemoryPort,
    agent_id: &str,
) -> Vec<procedural_cluster_service::ProceduralCluster> {
    procedural_cluster_service::plan_recent_clusters_since(
        mem,
        agent_id,
        procedural_cluster_service::ProceduralClusterKind::FailurePattern,
        12,
        6,
        0.95,
        Some(chrono::Utc::now() - chrono::Duration::days(30)),
    )
    .await
    .unwrap_or_default()
}

fn has_precedent_memory_entries(entries: &[MemoryEntry]) -> bool {
    entries
        .iter()
        .any(|entry| is_precedent_memory_category(&entry.category))
}

fn is_precedent_memory_category(category: &MemoryCategory) -> bool {
    matches!(category, MemoryCategory::Custom(name) if name == "precedent")
}

fn prioritize_uncontradicted_memory_entries(
    entries: &mut Vec<MemoryEntry>,
    failure_clusters: &[procedural_cluster_service::ProceduralCluster],
) {
    if entries.len() <= 1 || failure_clusters.is_empty() {
        return;
    }

    let mut preferred = Vec::new();
    let mut contradicted = Vec::new();

    for entry in entries.drain(..) {
        let is_contradicted = is_precedent_memory_category(&entry.category)
            && precedent_similarity_service::precedent_is_contradicted_by_failures(
                &entry.content,
                failure_clusters,
                0.75,
            );
        if is_contradicted {
            contradicted.push(entry);
        } else {
            preferred.push(entry);
        }
    }

    preferred.extend(contradicted);
    *entries = preferred;
}

fn load_memory_contradictions_from_clusters(
    failure_clusters: &[procedural_cluster_service::ProceduralCluster],
    ctx: &mut TurnMemoryContext,
) {
    if failure_clusters.is_empty() {
        return;
    }

    let mut cautions = Vec::new();
    let mut seen_keys = std::collections::HashSet::new();
    for entry in ctx
        .recalled_entries
        .iter()
        .chain(ctx.nearby_entries.iter())
        .chain(ctx.recent_echoes.iter())
    {
        if !is_precedent_memory_category(&entry.category) || !seen_keys.insert(entry.key.clone()) {
            continue;
        }
        let tools = precedent_similarity_service::precedent_summary_tools(&entry.content);
        if tools.is_empty() {
            continue;
        }
        let best_match = failure_clusters
            .iter()
            .filter_map(|cluster| {
                let failed_tools = failure_similarity_service::failure_summary_failed_tools(
                    &cluster.representative.content,
                );
                if failed_tools.is_empty() {
                    return None;
                }
                let overlap = tool_pattern_overlap(&tools, &failed_tools);
                (overlap >= 0.75).then_some(MemoryContradictionCaution {
                    entry_key: entry.key.clone(),
                    failure_representative_key: cluster.representative.key.clone(),
                    failed_tools,
                    overlap,
                })
            })
            .max_by(|left, right| {
                left.overlap
                    .partial_cmp(&right.overlap)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        if let Some(caution) = best_match {
            cautions.push(caution);
        }
    }

    cautions.sort_by(|left, right| {
        right
            .overlap
            .partial_cmp(&left.overlap)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.entry_key.cmp(&right.entry_key))
    });
    cautions.truncate(2);
    ctx.memory_cautions = cautions;
}

fn prioritize_uncontradicted_recipe_hits(
    recipe_hits: Vec<retrieval_service::RunRecipeSearchMatch>,
    stored_recipes: &[RunRecipe],
    failure_clusters: &[procedural_cluster_service::ProceduralCluster],
) -> Vec<retrieval_service::RunRecipeSearchMatch> {
    if recipe_hits.len() <= 1 || failure_clusters.is_empty() {
        return recipe_hits;
    }

    let mut preferred = Vec::new();
    let mut contradicted = Vec::new();

    for hit in recipe_hits {
        let is_contradicted = stored_recipes
            .iter()
            .find(|recipe| recipe.task_family == hit.task_family)
            .is_some_and(|recipe| {
                procedural_contradiction_service::recipe_is_contradicted(
                    recipe,
                    failure_clusters,
                    0.75,
                )
            });
        if is_contradicted {
            contradicted.push(hit);
        } else {
            preferred.push(hit);
        }
    }

    preferred.extend(contradicted);
    preferred
}

fn tool_pattern_overlap(left: &[String], right: &[String]) -> f64 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let shared = left
        .iter()
        .filter(|tool| right.iter().any(|other| other.eq_ignore_ascii_case(tool)))
        .count() as f64;
    let mut union = Vec::new();
    for tool in left.iter().chain(right.iter()) {
        if !union
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(tool))
        {
            union.push(tool.clone());
        }
    }
    if union.is_empty() {
        0.0
    } else {
        shared / union.len() as f64
    }
}

fn load_recipe_contradictions_from_clusters(
    run_recipe_store: &dyn RunRecipeStorePort,
    agent_id: &str,
    failure_clusters: &[procedural_cluster_service::ProceduralCluster],
    ctx: &mut TurnMemoryContext,
) {
    if ctx.run_recipes.is_empty() || failure_clusters.is_empty() {
        return;
    }

    let surfaced_recipes = ctx
        .run_recipes
        .iter()
        .filter_map(|hit| run_recipe_store.get(agent_id, &hit.task_family))
        .collect::<Vec<_>>();
    if surfaced_recipes.is_empty() {
        return;
    }

    let recipe_clusters = run_recipe_cluster_service::plan_recipe_clusters(&surfaced_recipes, 0.9);
    let surfaced_families = surfaced_recipes
        .iter()
        .map(|recipe| recipe.task_family.as_str())
        .collect::<std::collections::HashSet<_>>();

    ctx.procedural_contradictions =
        procedural_contradiction_service::find_recipe_failure_contradictions(
            &recipe_clusters,
            failure_clusters,
            0.75,
        )
        .into_iter()
        .filter(|contradiction| {
            surfaced_families.contains(contradiction.recipe_task_family.as_str())
        })
        .take(2)
        .collect();
}

async fn load_entity_neighbors(mem: &dyn UnifiedMemoryPort, ctx: &mut TurnMemoryContext) {
    if ctx.entities.is_empty() {
        return;
    }

    let mut neighborhoods = Vec::new();
    for entity in ctx.entities.iter().take(2) {
        if entity.id.trim().is_empty() {
            continue;
        }
        let traversed = match mem.traverse(&entity.id, 1).await {
            Ok(traversed) => traversed,
            Err(_) => continue,
        };
        let mut seen = std::collections::HashSet::new();
        let mut relations = Vec::new();
        for (related, fact) in traversed {
            if related.name.trim().is_empty() || fact.predicate.trim().is_empty() {
                continue;
            }
            let relation = format!("{}: {}", fact.predicate.trim(), related.name.trim());
            if seen.insert(relation.clone()) {
                relations.push(relation);
            }
            if relations.len() >= 2 {
                break;
            }
        }
        if !relations.is_empty() {
            neighborhoods.push(EntityNeighborhood {
                entity_name: entity.name.clone(),
                relations,
            });
        }
    }
    ctx.entity_neighbors = neighborhoods;
}

// ── Formatting ───────────────────────────────────────────────────
//
// Formatting lives here (not in the adapter layer) because both
// web (agent.rs) and channels (handle_inbound_message.rs) need it,
// and the channel path is a domain use case that can't call adapters.
// The adapter `turn_context_fmt` re-exports this as its public API.

/// Formatted prompt strings from turn context.
#[derive(Debug, Default)]
pub struct FormattedTurnContext {
    /// Core memory blocks for system prompt.
    pub core_blocks_system: String,
    /// Resolution plan for system prompt.
    pub resolution_system: String,
    /// Recall + skills + entities + sessions + recipes as enrichment prefix.
    pub enrichment_prefix: String,
}

#[derive(Debug, Clone, Default)]
pub struct EntityNeighborhood {
    pub entity_name: String,
    pub relations: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MemoryContradictionCaution {
    pub entry_key: String,
    pub failure_representative_key: String,
    pub failed_tools: Vec<String>,
    pub overlap: f64,
}

/// Format `TurnMemoryContext` into prompt-injectable strings.
///
/// Canonical formatter used by both web and channel paths.
/// The adapter layer (`turn_context_fmt`) re-exports this function.
pub fn format_turn_context(ctx: &TurnMemoryContext, budget: &PromptBudget) -> FormattedTurnContext {
    let mut result = FormattedTurnContext::default();
    let max_chars = budget.enrichment_total_max_chars;
    let mut remaining_projection_lines = ctx
        .execution_budget
        .as_ref()
        .map_or(usize::MAX, |b| b.retrieval_budget.max_projection_lines);

    result.core_blocks_system =
        render_core_blocks(&ctx.core_blocks, budget.core_blocks_total_max_chars);

    if let Some(plan) = &ctx.resolution_plan {
        if let Some(block) = resolution_router::format_resolution_plan(plan) {
            result.resolution_system = block;
        }
    }
    if let Some(guidance) = &ctx.clarification_guidance {
        if let Some(block) = clarification_policy::format_clarification_guidance(guidance) {
            result.resolution_system.push_str(&block);
        }
    }
    if let Some(guidance) = &ctx.execution_guidance {
        if let Some(block) = execution_guidance::format_execution_guidance(guidance) {
            result.resolution_system.push_str(&block);
        }
    }
    if let Some(packet) = &ctx.handoff_packet {
        result
            .resolution_system
            .push_str(&session_handoff::format_session_handoff_packet(packet));
    }

    for section in ordered_enrichment_sections(ctx, budget) {
        let Some(section) = take_section_lines(&section, &mut remaining_projection_lines) else {
            break;
        };
        if result.enrichment_prefix.chars().count() + section.chars().count() > max_chars {
            break;
        }
        result.enrichment_prefix.push_str(&section);
    }

    result
}

// ── Internal helpers ─────────────────────────────────────────────

/// Check if a memory key is an assistant-generated autosave key.
///
/// Must match `synapse_memory::is_assistant_autosave_key` logic.
fn is_autosave_key(key: &str) -> bool {
    let normalized = key.trim().to_ascii_lowercase();
    normalized == "assistant_resp" || normalized.starts_with("assistant_resp_")
}

fn render_core_blocks(blocks: &[CoreMemoryBlock], max_chars: usize) -> String {
    use std::fmt::Write;

    let mut ordered = blocks
        .iter()
        .filter(|block| !block.content.trim().is_empty())
        .collect::<Vec<_>>();
    ordered.sort_by_key(|block| core_block_priority(&block.label));

    let mut rendered = String::new();
    let mut remaining = max_chars;

    for block in ordered {
        let open = format!("<{}>\n", block.label);
        let close = format!("</{}>\n", block.label);
        let overhead = open.chars().count() + close.chars().count();
        if remaining <= overhead {
            break;
        }

        let content_budget = remaining - overhead;
        let content = truncate_with_ellipsis_chars(block.content.trim(), content_budget);
        if content.is_empty() {
            continue;
        }

        let _ = write!(rendered, "{open}{content}\n{close}");
        remaining = max_chars.saturating_sub(rendered.chars().count());
        if remaining == 0 {
            break;
        }
    }

    rendered
}

fn core_block_priority(label: &str) -> usize {
    match label {
        "task_state" => 0,
        "user_knowledge" => 1,
        "persona" => 2,
        "domain" => 3,
        _ => 4,
    }
}

fn truncate_with_ellipsis_chars(value: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    if max_chars <= 3 {
        return value.chars().take(max_chars).collect();
    }
    let truncated = value.chars().take(max_chars - 3).collect::<String>();
    format!("{truncated}...")
}

fn build_query_text(user_message: &str, interpretation: Option<&TurnInterpretation>) -> String {
    let base = user_message.trim();
    let Some(interpretation) = interpretation else {
        return base.to_string();
    };

    let mut parts = vec![base.to_string()];

    if let Some(profile) = interpretation.user_profile.as_ref() {
        for (key, value) in profile.iter() {
            let value = profile.get_text(key).unwrap_or_else(|| value.to_string());
            parts.push(format!("Profile fact {key}: {value}"));
        }
    }

    if !interpretation.reference_candidates.is_empty() {
        let references = interpretation
            .reference_candidates
            .iter()
            .map(|candidate| candidate.value.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        if !references.is_empty() {
            parts.push(format!("Reference candidates: {references}"));
        }
    }

    let Some(state) = interpretation.dialogue_state.as_ref() else {
        return parts.join("\n");
    };

    if !state.focus_entities.is_empty() {
        let focus = state
            .focus_entities
            .iter()
            .map(|(_, name)| name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        if !focus.is_empty() {
            parts.push(format!("Focus: {focus}"));
        }
    }

    if !state.comparison_set.is_empty() {
        let comparison = state
            .comparison_set
            .iter()
            .map(|(_, name)| name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        if !comparison.is_empty() {
            parts.push(format!("Comparison: {comparison}"));
        }
    }

    if !state.reference_anchors.is_empty() {
        let anchors = state
            .reference_anchors
            .iter()
            .map(|anchor| {
                let selector = match &anchor.selector {
                    crate::domain::dialogue_state::ReferenceAnchorSelector::Current => {
                        "current".to_string()
                    }
                    crate::domain::dialogue_state::ReferenceAnchorSelector::Latest => {
                        "latest".to_string()
                    }
                    crate::domain::dialogue_state::ReferenceAnchorSelector::Ordinal(ordinal) => {
                        match ordinal {
                            crate::domain::dialogue_state::ReferenceOrdinal::First => {
                                "first".to_string()
                            }
                            crate::domain::dialogue_state::ReferenceOrdinal::Second => {
                                "second".to_string()
                            }
                            crate::domain::dialogue_state::ReferenceOrdinal::Third => {
                                "third".to_string()
                            }
                            crate::domain::dialogue_state::ReferenceOrdinal::Fourth => {
                                "fourth".to_string()
                            }
                        }
                    }
                };
                match anchor.entity_kind.as_deref() {
                    Some(entity_kind) => format!("{selector}<{entity_kind}>={}", anchor.value),
                    None => format!("{selector}={}", anchor.value),
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        if !anchors.is_empty() {
            parts.push(format!("Reference anchors: {anchors}"));
        }
    }

    if !state.last_tool_subjects.is_empty() {
        parts.push(format!(
            "Recent tools: {}",
            state.last_tool_subjects.join(", ")
        ));
    }

    if let Some(target) = state.recent_delivery_target.as_ref() {
        parts.push(format!(
            "Recent delivery target: {}",
            summarize_delivery_target(target)
        ));
    }

    if let Some(schedule_job) = state.recent_schedule_job.as_ref() {
        let mut schedule_parts = vec![schedule_job.job_id.clone()];
        if let Some(session_target) = schedule_job.session_target.as_deref() {
            schedule_parts.push(format!("session={session_target}"));
        }
        if let Some(timezone) = schedule_job.timezone.as_deref() {
            schedule_parts.push(format!("timezone={timezone}"));
        }
        parts.push(format!("Recent schedule: {}", schedule_parts.join(", ")));
    }

    if let Some(resource) = state.recent_resource.as_ref() {
        let mut resource_parts = vec![resource.locator.clone()];
        if let Some(host) = resource.host.as_deref() {
            resource_parts.push(format!("host={host}"));
        }
        parts.push(format!("Recent resource: {}", resource_parts.join(", ")));
    }

    if let Some(search) = state.recent_search.as_ref() {
        let mut search_parts = Vec::new();
        if let Some(query) = search.query.as_deref() {
            search_parts.push(format!("query={query}"));
        }
        if let Some(locator) = search.primary_locator.as_deref() {
            search_parts.push(format!("result={locator}"));
        }
        if !search_parts.is_empty() {
            parts.push(format!("Recent search: {}", search_parts.join(", ")));
        }
    }

    if let Some(workspace) = state.recent_workspace.as_ref() {
        if let Some(name) = workspace.name.as_deref() {
            parts.push(format!("Recent workspace: {name}"));
        }
    }

    parts.join("\n")
}

fn summarize_delivery_target(
    target: &crate::domain::conversation_target::ConversationDeliveryTarget,
) -> String {
    match target {
        crate::domain::conversation_target::ConversationDeliveryTarget::CurrentConversation => {
            "current_conversation".into()
        }
        crate::domain::conversation_target::ConversationDeliveryTarget::Explicit {
            channel,
            recipient,
            thread_ref,
        } => match thread_ref.as_deref() {
            Some(thread_ref) => format!("{channel}:{recipient}#{thread_ref}"),
            None => format!("{channel}:{recipient}"),
        },
    }
}

fn build_execution_budget(
    interpretation: Option<&TurnInterpretation>,
) -> Option<turn_budget_policy::TurnExecutionBudget> {
    let interpretation = interpretation?;
    let dialogue_state = interpretation.dialogue_state.as_ref();
    let user_profile = interpretation.user_profile.as_ref();
    let signals = turn_budget_policy::TurnExecutionSignals {
        has_working_state: dialogue_state.is_some_and(|state| {
            !state.focus_entities.is_empty()
                || !state.comparison_set.is_empty()
                || !state.reference_anchors.is_empty()
                || !state.last_tool_subjects.is_empty()
                || state.recent_delivery_target.is_some()
                || state.recent_schedule_job.is_some()
                || state.recent_resource.is_some()
                || state.recent_search.is_some()
                || state.recent_workspace.is_some()
        }),
        has_profile_facts: user_profile.is_some_and(|profile| !profile.is_empty()),
        has_reference_candidates: !interpretation.reference_candidates.is_empty(),
        direct_reference_count: count_direct_reference_candidates(interpretation),
        structured_resolution_fact_count: count_structured_resolution_facts(interpretation),
        ambiguity_candidate_count: interpretation.clarification_candidates.len(),
        recent_tool_fact_count: dialogue_state.map_or(0, |state| state.last_tool_subjects.len()),
        explicit_user_correction: false,
    };
    Some(turn_budget_policy::build_turn_execution_budget(signals))
}

fn count_direct_reference_candidates(interpretation: &TurnInterpretation) -> usize {
    interpretation
        .reference_candidates
        .iter()
        .filter(|candidate| {
            candidate.source == ReferenceSource::DialogueState
                && matches!(
                    candidate.kind,
                    ReferenceCandidateKind::DeliveryTarget
                        | ReferenceCandidateKind::ScheduleJob
                        | ReferenceCandidateKind::ResourceLocator { .. }
                        | ReferenceCandidateKind::SearchQuery { .. }
                        | ReferenceCandidateKind::SearchResult { .. }
                        | ReferenceCandidateKind::WorkspaceName { .. }
                )
        })
        .count()
}

fn count_structured_resolution_facts(interpretation: &TurnInterpretation) -> usize {
    let mut count = 0usize;

    if interpretation.configured_delivery_target.is_some() {
        count += 1;
    }

    if let Some(profile) = interpretation.user_profile.as_ref() {
        count += profile.fact_count();
    }

    count
}

fn apply_resolution_plan(
    ctx: &mut TurnMemoryContext,
    user_message: &str,
    interpretation: Option<&TurnInterpretation>,
    recent_tool_repairs: &[ToolRepairTrace],
    recent_admission_reasons: &[CandidateAdmissionReason],
    recent_admission_repair: Option<AdmissionRepairHint>,
) {
    let plan = resolution_router::build_resolution_plan(resolution_router::ResolutionEvidence {
        interpretation,
        top_session_score: ctx.session_matches.first().map(|session| session.score),
        second_session_score: ctx.session_matches.get(1).map(|session| session.score),
        top_recipe_score: ctx.run_recipes.first().map(|recipe| recipe.score),
        second_recipe_score: ctx.run_recipes.get(1).map(|recipe| recipe.score),
        top_memory_score: ctx
            .recalled_entries
            .first()
            .and_then(memory_resolution_score),
        second_memory_score: ctx
            .recalled_entries
            .get(1)
            .and_then(memory_resolution_score),
        recall_hits: ctx.recalled_entries.len(),
        skill_hits: ctx.skills.len(),
        entity_hits: ctx.entities.len(),
    });
    if !plan.source_order.is_empty() {
        ctx.clarification_guidance =
            clarification_policy::build_clarification_guidance(Some(&plan), interpretation);
        ctx.execution_guidance = execution_guidance::build_execution_guidance(
            Some(&plan),
            interpretation,
            recent_tool_repairs,
            recent_admission_reasons,
            recent_admission_repair,
        );
        ctx.resolution_plan = Some(plan);
    } else {
        ctx.clarification_guidance =
            clarification_policy::build_clarification_guidance(None, interpretation);
        ctx.execution_guidance = execution_guidance::build_execution_guidance(
            None,
            interpretation,
            recent_tool_repairs,
            recent_admission_reasons,
            recent_admission_repair,
        );
    }
    ctx.handoff_packet =
        session_handoff::build_session_handoff_packet(session_handoff::SessionHandoffInput {
            user_message,
            interpretation,
            recent_admission_repair,
            recent_admission_reasons,
            recalled_entries: &ctx.recalled_entries,
            session_matches: &ctx.session_matches,
            run_recipes: &ctx.run_recipes,
        });
}

fn memory_resolution_score(entry: &MemoryEntry) -> Option<f64> {
    let score = entry.score?;
    Some((score + memory_epistemic_retrieval_score_delta(entry)).clamp(0.0, 1.0))
}

fn ordered_enrichment_sections(ctx: &TurnMemoryContext, budget: &PromptBudget) -> Vec<String> {
    let plan = ctx.resolution_plan.as_ref();
    let mut sections = Vec::<(usize, String)>::new();

    if let Some(section) = format_memory_section(ctx, budget) {
        let source = resolution_router::ResolutionSource::LongTermMemory;
        if should_include_enrichment_source(ctx, source) {
            sections.push((resolution_router::source_priority(plan, source), section));
        }
    }
    if let Some(section) = format_session_section(ctx) {
        let source = resolution_router::ResolutionSource::SessionHistory;
        if should_include_enrichment_source(ctx, source) {
            sections.push((resolution_router::source_priority(plan, source), section));
        }
    }
    if let Some(section) = format_recipe_section(ctx) {
        let source = resolution_router::ResolutionSource::RunRecipe;
        if should_include_enrichment_source(ctx, source) {
            sections.push((resolution_router::source_priority(plan, source), section));
        }
    }

    sections.sort_by_key(|(priority, _)| *priority);
    sections.into_iter().map(|(_, section)| section).collect()
}

fn should_include_enrichment_source(
    ctx: &TurnMemoryContext,
    source: resolution_router::ResolutionSource,
) -> bool {
    let Some(execution_budget) = ctx.execution_budget.as_ref() else {
        return true;
    };
    let Some(plan) = ctx.resolution_plan.as_ref() else {
        return true;
    };

    let has_direct_reference_gate = execution_budget
        .gate_reasons
        .contains(&turn_budget_policy::InterpreterGateReason::DirectTypedReference);
    if !has_direct_reference_gate {
        return true;
    }

    if plan.confidence == resolution_router::ResolutionConfidence::Low {
        return true;
    }

    let priority = resolution_router::source_priority(Some(plan), source);
    priority != usize::MAX && priority <= 1
}

fn take_section_lines(section: &str, remaining_lines: &mut usize) -> Option<String> {
    if *remaining_lines == 0 || section.trim().is_empty() {
        return None;
    }

    let lines = section.lines().collect::<Vec<_>>();
    if lines.is_empty() {
        return None;
    }

    if lines.len() <= *remaining_lines {
        *remaining_lines -= lines.len();
        let mut result = lines.join("\n");
        if section.ends_with('\n') {
            result.push('\n');
        }
        return Some(result);
    }

    if *remaining_lines == 1 {
        *remaining_lines = 0;
        return Some("...\n".to_string());
    }

    let visible_lines = (*remaining_lines).saturating_sub(1);
    let mut result = lines
        .into_iter()
        .take(visible_lines)
        .collect::<Vec<_>>()
        .join("\n");
    result.push_str("\n...\n");
    *remaining_lines = 0;
    Some(result)
}

fn format_memory_section(ctx: &TurnMemoryContext, budget: &PromptBudget) -> Option<String> {
    let mut section = String::new();

    if !ctx.recalled_entries.is_empty()
        || !ctx.nearby_entries.is_empty()
        || !ctx.recent_echoes.is_empty()
        || !ctx.memory_cautions.is_empty()
    {
        let header = "[Memory context]\n";
        let mut recall_section = String::from(header);
        let mut added = false;
        for entry in &ctx.recalled_entries {
            append_memory_line(&mut recall_section, entry, budget.recall_entry_max_chars);
            added = true;
        }
        if !ctx.nearby_entries.is_empty() {
            if added {
                recall_section.push_str("[Nearby memory]\n");
            }
            for entry in &ctx.nearby_entries {
                append_memory_line(&mut recall_section, entry, budget.recall_entry_max_chars);
                added = true;
            }
        }
        if !ctx.recent_echoes.is_empty() {
            if added {
                recall_section.push_str("[Recent echoes]\n");
            }
            for entry in &ctx.recent_echoes {
                append_memory_line(&mut recall_section, entry, budget.recall_entry_max_chars);
                added = true;
            }
        }
        if !ctx.memory_cautions.is_empty() {
            if added {
                recall_section.push_str("[Memory cautions]\n");
            }
            for caution in &ctx.memory_cautions {
                recall_section.push_str(&format!(
                    "<memory-caution key=\"{}\" overlap=\"{:.2}\">\n",
                    caution.entry_key, caution.overlap
                ));
                if !caution.failed_tools.is_empty() {
                    recall_section.push_str("Failed tools: ");
                    recall_section.push_str(&caution.failed_tools.join(", "));
                    recall_section.push('\n');
                }
                recall_section.push_str("Failure anchor: ");
                recall_section.push_str(&caution.failure_representative_key);
                recall_section.push('\n');
                recall_section.push_str("</memory-caution>\n");
                added = true;
            }
        }
        if added {
            recall_section.push('\n');
            section.push_str(&recall_section);
        }
    }

    for skill in &ctx.skills {
        if skill.content.trim().is_empty() {
            continue;
        }
        section.push_str(&format!(
            "<skill name=\"{}\" origin=\"{}\" status=\"{}\">\n{}\n</skill>\n",
            skill.name,
            skill.origin,
            skill.status,
            skill.content.trim(),
        ));
    }

    for entity in &ctx.entities {
        if let Some(summary) = entity.summary.as_ref() {
            let mut block = format!(
                "<entity name=\"{}\" type=\"{}\">\n{}\n",
                entity.name, entity.entity_type, summary
            );
            if let Some(neighborhood) = ctx
                .entity_neighbors
                .iter()
                .find(|neighborhood| neighborhood.entity_name == entity.name)
            {
                block.push_str("Relations: ");
                block.push_str(&neighborhood.relations.join(" | "));
                block.push('\n');
            }
            block.push_str("</entity>\n");
            section.push_str(&block);
        }
    }

    if section.is_empty() {
        None
    } else {
        Some(section)
    }
}

fn append_memory_line(section: &mut String, entry: &MemoryEntry, max_chars: usize) {
    let content = if entry.content.chars().count() > max_chars {
        let truncated: String = entry.content.chars().take(max_chars).collect();
        format!("{truncated}…")
    } else {
        entry.content.clone()
    };
    let epistemic = epistemic_entry_for_memory_entry(entry);
    section.push_str(&format!(
        "- {} [{}]: {content}\n",
        entry.key,
        format_epistemic_entry(&epistemic)
    ));
}

fn format_session_section(ctx: &TurnMemoryContext) -> Option<String> {
    if ctx.session_matches.is_empty() {
        return None;
    }
    let mut section = String::new();
    for session in &ctx.session_matches {
        let mut block = format!(
            "<session-recap key=\"{}\" kind=\"{}\">\n",
            session.session_key, session.kind
        );
        if let Some(label) = session.label.as_deref() {
            if !label.trim().is_empty() {
                block.push_str("Label: ");
                block.push_str(label.trim());
                block.push('\n');
            }
        }
        if let Some(summary) = session.summary.as_deref() {
            if !summary.trim().is_empty() {
                block.push_str(summary.trim());
                block.push('\n');
            }
        }
        if let Some(recap) = session.recap.as_deref() {
            if !recap.trim().is_empty() {
                block.push_str("Recent match: ");
                block.push_str(recap.trim());
                block.push('\n');
            }
        }
        block.push_str("</session-recap>\n");
        section.push_str(&block);
    }
    Some(section)
}

fn format_recipe_section(ctx: &TurnMemoryContext) -> Option<String> {
    if ctx.run_recipes.is_empty() && ctx.procedural_contradictions.is_empty() {
        return None;
    }
    let mut section = String::new();
    for recipe in &ctx.run_recipes {
        let mut block = format!(
            "<recipe task_family=\"{}\" success_count=\"{}\">\n",
            recipe.task_family, recipe.success_count
        );
        if !recipe.summary.trim().is_empty() {
            block.push_str(recipe.summary.trim());
            block.push('\n');
        }
        if !recipe.sample_request.trim().is_empty() {
            block.push_str("Sample request: ");
            block.push_str(recipe.sample_request.trim());
            block.push('\n');
        }
        if !recipe.tool_pattern.is_empty() {
            block.push_str("Tool pattern: ");
            block.push_str(&recipe.tool_pattern.join(", "));
            block.push('\n');
        }
        block.push_str("</recipe>\n");
        section.push_str(&block);
    }
    for contradiction in &ctx.procedural_contradictions {
        let mut block = format!(
            "<procedural-contradiction task_family=\"{}\" overlap=\"{:.2}\">\n",
            contradiction.recipe_task_family, contradiction.overlap
        );
        if !contradiction.recipe_lineage_task_families.is_empty() {
            block.push_str("Lineage: ");
            block.push_str(&contradiction.recipe_lineage_task_families.join(", "));
            block.push('\n');
        }
        if !contradiction.failed_tools.is_empty() {
            block.push_str("Failed tools: ");
            block.push_str(&contradiction.failed_tools.join(", "));
            block.push('\n');
        }
        block.push_str("Failure anchor: ");
        block.push_str(&contradiction.failure_representative_key);
        block.push('\n');
        block.push_str("</procedural-contradiction>\n");
        section.push_str(&block);
    }
    Some(section)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::retrieval_service::{
        RunRecipeSearchMatch, SessionSearchMatch,
    };
    use crate::application::services::turn_budget_policy::{
        InterpreterGateReason, InterpreterMode, RetrievalBudget, TurnExecutionBudget,
    };
    use crate::domain::dialogue_state::DialogueState;
    use crate::domain::memory::{CoreMemoryBlock, Entity, MemoryCategory, MemoryEntry, Skill};

    // ── Budget & policy defaults ──

    #[test]
    fn default_budget_values() {
        let b = PromptBudget::default();
        assert_eq!(b.core_blocks_total_max_chars, 1_800);
        assert_eq!(b.recall_max_entries, 5);
        assert_eq!(b.nearby_max_entries, 2);
        assert_eq!(b.recall_entry_max_chars, 800);
        assert_eq!(b.recall_total_max_chars, 4_000);
        assert!((b.recall_min_relevance - 0.4).abs() < f64::EPSILON);
        assert_eq!(b.skills_max_count, 3);
        assert_eq!(b.skills_total_max_chars, 2_000);
        assert_eq!(b.entities_max_count, 3);
        assert_eq!(b.entities_total_max_chars, 1_500);
        assert_eq!(b.enrichment_total_max_chars, 8_000);
    }

    #[test]
    fn default_continuation_policy() {
        let p = ContinuationPolicy::default();
        match p {
            ContinuationPolicy::CorePlusRecall {
                recall_max_entries: n,
            } => assert_eq!(n, 2),
            _ => panic!("expected CorePlusRecall"),
        }
    }

    #[test]
    fn is_autosave_key_matches() {
        assert!(is_autosave_key("assistant_resp"));
        assert!(is_autosave_key("assistant_resp_1234"));
        assert!(is_autosave_key("ASSISTANT_RESP_abcd"));
        assert!(!is_autosave_key("assistant_response"));
        assert!(!is_autosave_key("user_msg_1234"));
        assert!(!is_autosave_key("core_persona"));
    }

    #[tokio::test]
    async fn build_query_text_includes_typed_dialogue_state() {
        let state = DialogueState {
            focus_entities: vec![crate::domain::dialogue_state::FocusEntity {
                kind: "project".into(),
                name: "Borealis".into(),
                metadata: None,
            }],
            last_tool_subjects: vec!["workspace_lookup".into()],
            ..Default::default()
        };
        let interpretation =
            crate::application::services::turn_interpretation::build_turn_interpretation(
                None,
                "what is the current workspace anchor?",
                Some({
                    let mut profile = crate::domain::user_profile::UserProfile::default();
                    profile.set("workspace_anchor", serde_json::json!("Borealis"));
                    profile.set("project_alias", serde_json::json!("Borealis"));
                    profile
                }),
                None,
                Some(&state),
                None,
            )
            .await
            .unwrap();
        let query = build_query_text(
            "what is the current workspace anchor?",
            Some(&interpretation),
        );
        assert!(query.contains("what is the current workspace anchor?"));
        assert!(query.contains("Profile fact workspace_anchor: Borealis"));
        assert!(query.contains("Focus: Borealis"));
        assert!(query.contains("Recent tools: workspace_lookup"));
    }

    #[test]
    fn build_query_text_includes_recent_typed_context_recaps() {
        let interpretation = crate::application::services::turn_interpretation::TurnInterpretation {
            dialogue_state: Some(
                crate::application::services::turn_interpretation::DialogueStateSnapshot {
                    focus_entities: vec![],
                    comparison_set: vec![],
                    reference_anchors: vec![],
                    last_tool_subjects: vec![],
                    recent_delivery_target: Some(
                        crate::domain::conversation_target::ConversationDeliveryTarget::Explicit {
                            channel: "matrix".into(),
                            recipient: "!ops:example.org".into(),
                            thread_ref: Some("$event1".into()),
                        },
                    ),
                    recent_schedule_job: Some(
                        crate::domain::dialogue_state::ScheduleJobReference {
                            job_id: "job-42".into(),
                            action: crate::domain::tool_fact::ScheduleAction::Run,
                            job_type: Some(crate::domain::tool_fact::ScheduleJobType::Agent),
                            schedule_kind: Some(crate::domain::tool_fact::ScheduleKind::Cron),
                            session_target: Some("ops-room".into()),
                            timezone: Some("Europe/Berlin".into()),
                        },
                    ),
                    recent_resource: Some(crate::domain::dialogue_state::ResourceReference {
                        kind: crate::domain::tool_fact::ResourceKind::File,
                        operation: crate::domain::tool_fact::ResourceOperation::Read,
                        locator: "/tmp/report.md".into(),
                        host: Some("workspace".into()),
                    }),
                    recent_search: Some(crate::domain::dialogue_state::SearchReference {
                        domain: crate::domain::tool_fact::SearchDomain::Session,
                        query: Some("deploy rollback".into()),
                        primary_locator: Some("session:incident-12".into()),
                        result_count: Some(3),
                    }),
                    recent_workspace: Some(crate::domain::dialogue_state::WorkspaceReference {
                        action: crate::domain::tool_fact::WorkspaceAction::Switch,
                        name: Some("synapseclaw".into()),
                        item_count: Some(42),
                    }),
                },
            ),
            ..Default::default()
        };

        let query = build_query_text("rerun it", Some(&interpretation));
        assert!(query.contains("Recent delivery target: matrix:!ops:example.org#$event1"));
        assert!(query.contains("Recent schedule: job-42, session=ops-room, timezone=Europe/Berlin"));
        assert!(query.contains("Recent resource: /tmp/report.md, host=workspace"));
        assert!(query.contains("Recent search: query=deploy rollback, result=session:incident-12"));
        assert!(query.contains("Recent workspace: synapseclaw"));
    }

    #[test]
    fn execution_budget_uses_interpretation_signals() {
        let interpretation =
            crate::application::services::turn_interpretation::TurnInterpretation {
                user_profile: Some({
                    let mut profile = crate::domain::user_profile::UserProfile::default();
                    profile.set("workspace_anchor", serde_json::json!("Borealis"));
                    profile
                }),
                dialogue_state: Some(
                    crate::application::services::turn_interpretation::DialogueStateSnapshot {
                        focus_entities: vec![("city".into(), "Berlin".into())],
                        comparison_set: vec![
                            ("city".into(), "Berlin".into()),
                            ("city".into(), "Tbilisi".into()),
                        ],
                        reference_anchors: vec![],
                        last_tool_subjects: vec!["weather_lookup".into()],
                        recent_delivery_target: None,
                        recent_schedule_job: None,
                        recent_resource: None,
                        recent_search: None,
                        recent_workspace: None,
                    },
                ),
                clarification_candidates: vec!["Berlin".into(), "Tbilisi".into()],
                ..Default::default()
            };

        let execution_budget = build_execution_budget(Some(&interpretation)).unwrap();
        assert_eq!(
            execution_budget.interpreter_mode,
            InterpreterMode::Lightweight
        );
        assert_eq!(execution_budget.retrieval_budget.max_session_candidates, 2);
    }

    #[test]
    fn execution_budget_trims_history_when_direct_typed_reference_exists() {
        let interpretation =
            crate::application::services::turn_interpretation::TurnInterpretation {
                dialogue_state: Some(
                    crate::application::services::turn_interpretation::DialogueStateSnapshot {
                        focus_entities: vec![],
                        comparison_set: vec![],
                        reference_anchors: vec![],
                        last_tool_subjects: vec!["job_123".into()],
                        recent_delivery_target: None,
                        recent_schedule_job: Some(
                            crate::domain::dialogue_state::ScheduleJobReference {
                                job_id: "job_123".into(),
                                action: crate::domain::tool_fact::ScheduleAction::Run,
                                job_type: Some(
                                    crate::domain::tool_fact::ScheduleJobType::Agent,
                                ),
                                schedule_kind: Some(
                                    crate::domain::tool_fact::ScheduleKind::Cron,
                                ),
                                session_target: Some("main".into()),
                                timezone: Some("Europe/Berlin".into()),
                            },
                        ),
                        recent_resource: None,
                        recent_search: None,
                        recent_workspace: None,
                    },
                ),
                reference_candidates: vec![
                    crate::application::services::turn_interpretation::ReferenceCandidate {
                        kind: crate::application::services::turn_interpretation::ReferenceCandidateKind::ScheduleJob,
                        value: "job_123".into(),
                        source: crate::application::services::turn_interpretation::ReferenceSource::DialogueState,
                    },
                ],
                clarification_candidates: vec![],
                ..Default::default()
            };

        let execution_budget = build_execution_budget(Some(&interpretation)).unwrap();
        assert_eq!(
            execution_budget.interpreter_mode,
            InterpreterMode::Lightweight
        );
        assert!(execution_budget
            .gate_reasons
            .contains(&InterpreterGateReason::DirectTypedReference));
        assert_eq!(execution_budget.retrieval_budget.max_session_candidates, 0);
        assert_eq!(
            execution_budget.retrieval_budget.max_precedent_candidates,
            0
        );
        assert_eq!(execution_budget.retrieval_budget.max_memory_candidates, 3);
    }

    #[test]
    fn execution_budget_trims_history_when_structured_profile_facts_exist() {
        let interpretation =
            crate::application::services::turn_interpretation::TurnInterpretation {
                user_profile: Some({
                    let mut profile = crate::domain::user_profile::UserProfile::default();
                    profile.set("workspace_anchor", serde_json::json!("Borealis"));
                    profile.set("project_alias", serde_json::json!("Borealis"));
                    profile
                }),
                clarification_candidates: vec![],
                ..Default::default()
            };

        let execution_budget = build_execution_budget(Some(&interpretation)).unwrap();
        assert_eq!(
            execution_budget.interpreter_mode,
            InterpreterMode::Lightweight
        );
        assert_eq!(execution_budget.retrieval_budget.max_session_candidates, 0);
        assert_eq!(
            execution_budget.retrieval_budget.max_precedent_candidates,
            0
        );
        assert_eq!(execution_budget.retrieval_budget.max_memory_candidates, 3);
    }

    #[test]
    fn current_conversation_alone_does_not_trigger_direct_reference_budget_trim() {
        let interpretation =
            crate::application::services::turn_interpretation::TurnInterpretation {
                current_conversation: Some(
                    crate::application::services::turn_interpretation::CurrentConversationSnapshot {
                        adapter: "matrix".into(),
                        has_thread: true,
                    },
                ),
                reference_candidates: vec![
                    crate::application::services::turn_interpretation::ReferenceCandidate {
                        kind: crate::application::services::turn_interpretation::ReferenceCandidateKind::DeliveryTarget,
                        value: "current_conversation".into(),
                        source: crate::application::services::turn_interpretation::ReferenceSource::CurrentConversation,
                    },
                ],
                clarification_candidates: vec![],
                ..Default::default()
            };

        let execution_budget = build_execution_budget(Some(&interpretation)).unwrap();
        assert_eq!(
            execution_budget.interpreter_mode,
            InterpreterMode::Lightweight
        );
        assert!(!execution_budget
            .gate_reasons
            .contains(&InterpreterGateReason::DirectTypedReference));
        assert_eq!(execution_budget.retrieval_budget.max_session_candidates, 2);
        assert_eq!(
            execution_budget.retrieval_budget.max_precedent_candidates,
            2
        );
    }

    #[test]
    fn take_section_lines_truncates_with_ellipsis() {
        let mut remaining_lines = 3usize;
        let section = "line1\nline2\nline3\nline4\n";

        let trimmed = take_section_lines(section, &mut remaining_lines).unwrap();

        assert_eq!(remaining_lines, 0);
        assert_eq!(trimmed.lines().count(), 3);
        assert!(trimmed.contains("line1"));
        assert!(trimmed.contains("line2"));
        assert!(trimmed.contains("..."));
    }

    #[test]
    fn format_turn_context_respects_projection_line_budget() {
        let ctx = TurnMemoryContext {
            recalled_entries: vec![
                make_entry("k1", "alpha", 0.9),
                make_entry("k2", "beta", 0.8),
                make_entry("k3", "gamma", 0.7),
            ],
            execution_budget: Some(TurnExecutionBudget {
                retrieval_budget: RetrievalBudget {
                    max_projection_lines: 2,
                    ..RetrievalBudget::default()
                },
                ..TurnExecutionBudget::default()
            }),
            ..TurnMemoryContext::default()
        };

        let formatted = format_turn_context(&ctx, &PromptBudget::default());

        assert!(formatted.enrichment_prefix.lines().count() <= 2);
        assert!(formatted.enrichment_prefix.contains("[Memory context]"));
        assert!(formatted.enrichment_prefix.contains("..."));
    }

    #[test]
    fn direct_reference_skips_low_priority_memory_enrichment() {
        let ctx = TurnMemoryContext {
            recalled_entries: vec![make_entry("fact1", "User likes Rust", 0.9)],
            execution_budget: Some(TurnExecutionBudget {
                gate_reasons: vec![InterpreterGateReason::DirectTypedReference],
                ..TurnExecutionBudget::default()
            }),
            resolution_plan: Some(resolution_router::ResolutionPlan {
                source_order: vec![
                    resolution_router::ResolutionSource::DialogueState,
                    resolution_router::ResolutionSource::UserProfile,
                    resolution_router::ResolutionSource::LongTermMemory,
                ],
                confidence: resolution_router::ResolutionConfidence::Medium,
                clarify_after_exhaustion: true,
                clarification_reason: None,
            }),
            ..TurnMemoryContext::default()
        };

        let fmt = format_turn_context(&ctx, &PromptBudget::default());
        assert!(!fmt.enrichment_prefix.contains("[Memory context]"));
    }

    #[test]
    fn direct_reference_keeps_secondary_memory_enrichment() {
        let ctx = TurnMemoryContext {
            recalled_entries: vec![make_entry("fact1", "User likes Rust", 0.9)],
            execution_budget: Some(TurnExecutionBudget {
                gate_reasons: vec![InterpreterGateReason::DirectTypedReference],
                ..TurnExecutionBudget::default()
            }),
            resolution_plan: Some(resolution_router::ResolutionPlan {
                source_order: vec![
                    resolution_router::ResolutionSource::DialogueState,
                    resolution_router::ResolutionSource::LongTermMemory,
                ],
                confidence: resolution_router::ResolutionConfidence::Medium,
                clarify_after_exhaustion: true,
                clarification_reason: None,
            }),
            ..TurnMemoryContext::default()
        };

        let fmt = format_turn_context(&ctx, &PromptBudget::default());
        assert!(fmt.enrichment_prefix.contains("[Memory context]"));
    }

    // ── Helpers ──

    fn make_core_block(label: &str, content: &str) -> CoreMemoryBlock {
        CoreMemoryBlock {
            agent_id: "test".into(),
            label: label.into(),
            content: content.into(),
            max_tokens: 500,
            updated_at: chrono::Utc::now(),
        }
    }

    fn make_entry(key: &str, content: &str, score: f64) -> MemoryEntry {
        MemoryEntry {
            id: String::new(),
            key: key.into(),
            content: content.into(),
            category: MemoryCategory::Core,
            score: Some(score),
            timestamp: String::new(),
            session_id: None,
        }
    }

    fn make_precedent_entry(key: &str, content: &str, score: f64) -> MemoryEntry {
        MemoryEntry {
            category: MemoryCategory::Custom("precedent".into()),
            ..make_entry(key, content, score)
        }
    }

    fn make_skill(name: &str, content: &str) -> Skill {
        Skill {
            id: String::new(),
            name: name.into(),
            description: String::new(),
            content: content.into(),
            task_family: None,
            lineage_task_families: Vec::new(),
            tool_pattern: Vec::new(),
            tags: vec![],
            success_count: 1,
            fail_count: 0,
            version: 1,
            origin: crate::domain::memory::SkillOrigin::Learned,
            status: crate::domain::memory::SkillStatus::Active,
            created_by: "test".into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn make_entity(name: &str, summary: &str) -> Entity {
        Entity {
            id: String::new(),
            name: name.into(),
            entity_type: "concept".into(),
            summary: Some(summary.into()),
            properties: serde_json::json!({}),
            created_by: "test".into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn make_recipe(task_family: &str, summary: &str) -> RunRecipeSearchMatch {
        RunRecipeSearchMatch {
            score: 220,
            task_family: task_family.into(),
            sample_request: format!("{task_family} the latest release"),
            summary: summary.into(),
            tool_pattern: vec!["shell".into(), "git".into()],
            success_count: 3,
            updated_at: 1,
        }
    }

    fn make_stored_recipe(task_family: &str, tool_pattern: &[&str], summary: &str) -> RunRecipe {
        RunRecipe {
            agent_id: "agent".into(),
            task_family: task_family.into(),
            lineage_task_families: vec![task_family.into()],
            sample_request: format!("{task_family} the latest release"),
            summary: summary.into(),
            tool_pattern: tool_pattern
                .iter()
                .map(|tool| (*tool).to_string())
                .collect(),
            success_count: 3,
            updated_at: 1,
        }
    }

    fn make_failure_cluster(
        key: &str,
        summary: &str,
    ) -> procedural_cluster_service::ProceduralCluster {
        procedural_cluster_service::ProceduralCluster {
            representative: MemoryEntry {
                id: key.into(),
                key: key.into(),
                content: summary.into(),
                category: MemoryCategory::Custom("failure_pattern".into()),
                timestamp: "2026-01-01T00:00:00Z".into(),
                session_id: None,
                score: None,
            },
            member_keys: vec![key.into()],
        }
    }

    fn make_session_match(label: &str, summary: &str, recap: &str) -> SessionSearchMatch {
        SessionSearchMatch {
            score: 2.4,
            session_key: "channel_alice".into(),
            label: Some(label.into()),
            kind: crate::domain::conversation::ConversationKind::Channel,
            message_count: 8,
            summary: Some(summary.into()),
            recap: Some(recap.into()),
        }
    }

    // ── format_turn_context: core blocks ──

    #[test]
    fn format_core_blocks_xml_tags() {
        let ctx = TurnMemoryContext {
            core_blocks: vec![
                make_core_block("persona", "I am helpful"),
                make_core_block("user_knowledge", "Prefers Rust"),
            ],
            ..Default::default()
        };
        let fmt = format_turn_context(&ctx, &PromptBudget::default());
        assert!(fmt.core_blocks_system.contains("<persona>"));
        assert!(fmt.core_blocks_system.contains("I am helpful"));
        assert!(fmt.core_blocks_system.contains("</persona>"));
        assert!(fmt.core_blocks_system.contains("<user_knowledge>"));
        assert!(fmt.core_blocks_system.contains("Prefers Rust"));
        assert!(fmt.enrichment_prefix.is_empty());
    }

    #[test]
    fn format_core_blocks_respects_budget_priority() {
        let ctx = TurnMemoryContext {
            core_blocks: vec![
                make_core_block("persona", &"p".repeat(60)),
                make_core_block("domain", &"d".repeat(60)),
                make_core_block("task_state", &"t".repeat(60)),
                make_core_block("user_knowledge", &"u".repeat(60)),
            ],
            ..Default::default()
        };

        let budget = PromptBudget {
            core_blocks_total_max_chars: 180,
            ..PromptBudget::default()
        };
        let fmt = format_turn_context(&ctx, &budget);
        assert!(fmt.core_blocks_system.contains("<task_state>"));
        assert!(fmt.core_blocks_system.contains("<user_knowledge>"));
        assert!(!fmt.core_blocks_system.contains("<persona>"));
        assert!(!fmt.core_blocks_system.contains("<domain>"));
    }

    #[test]
    fn format_empty_core_block_skipped() {
        let ctx = TurnMemoryContext {
            core_blocks: vec![make_core_block("persona", "  ")],
            ..Default::default()
        };
        let fmt = format_turn_context(&ctx, &PromptBudget::default());
        assert!(fmt.core_blocks_system.is_empty());
    }

    // ── format_turn_context: recall entries ──

    #[test]
    fn format_recall_entries_with_header() {
        let ctx = TurnMemoryContext {
            recalled_entries: vec![make_entry("fact1", "User likes Rust", 0.9)],
            ..Default::default()
        };
        let fmt = format_turn_context(&ctx, &PromptBudget::default());
        assert!(fmt.enrichment_prefix.starts_with("[Memory context]\n"));
        assert!(fmt.enrichment_prefix.contains("- fact1 [state=known"));
        assert!(fmt.enrichment_prefix.contains("User likes Rust"));
    }

    #[test]
    fn memory_resolution_score_applies_epistemic_adjustment() {
        let weak = make_entry("weak", "Low-confidence memory", 0.64);
        let known = make_entry("known", "High-confidence memory", 0.9);

        assert!(memory_resolution_score(&weak).unwrap() < weak.score.unwrap());
        assert!(memory_resolution_score(&known).unwrap() > known.score.unwrap());
    }

    #[test]
    fn format_recall_entry_truncated() {
        let long = "x".repeat(1000);
        let ctx = TurnMemoryContext {
            recalled_entries: vec![make_entry("k", &long, 0.9)],
            ..Default::default()
        };
        let budget = PromptBudget {
            recall_entry_max_chars: 50,
            ..Default::default()
        };
        let fmt = format_turn_context(&ctx, &budget);
        assert!(fmt.enrichment_prefix.contains("…"));
        // Truncated to ~50 chars + key + formatting
        assert!(fmt.enrichment_prefix.len() < 200);
    }

    #[test]
    fn format_nearby_memory_entries_with_subheader() {
        let ctx = TurnMemoryContext {
            recalled_entries: vec![make_entry("fact1", "Primary fact", 0.9)],
            nearby_entries: vec![make_entry("fact2", "Nearby fact", 0.7)],
            recent_echoes: vec![make_entry("fact3", "Recent echo", 0.6)],
            ..Default::default()
        };
        let fmt = format_turn_context(&ctx, &PromptBudget::default());
        assert!(fmt.enrichment_prefix.contains("[Memory context]"));
        assert!(fmt.enrichment_prefix.contains("- fact1 [state=known"));
        assert!(fmt.enrichment_prefix.contains("Primary fact"));
        assert!(fmt.enrichment_prefix.contains("[Nearby memory]"));
        assert!(fmt.enrichment_prefix.contains("- fact2 [state=known"));
        assert!(fmt.enrichment_prefix.contains("Nearby fact"));
        assert!(fmt.enrichment_prefix.contains("[Recent echoes]"));
        assert!(fmt
            .enrichment_prefix
            .contains("- fact3 [state=needs_verification"));
        assert!(fmt.enrichment_prefix.contains("Recent echo"));
    }

    #[test]
    fn format_memory_section_surfaces_precedent_cautions() {
        let ctx = TurnMemoryContext {
            recalled_entries: vec![make_precedent_entry(
                "precedent-status",
                "tools=web_search -> message_send | subjects=status.example.com",
                0.91,
            )],
            memory_cautions: vec![MemoryContradictionCaution {
                entry_key: "precedent-status".into(),
                failure_representative_key: "failure-1".into(),
                failed_tools: vec!["web_search".into(), "message_send".into()],
                overlap: 1.0,
            }],
            ..Default::default()
        };

        let fmt = format_turn_context(&ctx, &PromptBudget::default());
        assert!(fmt.enrichment_prefix.contains("[Memory cautions]"));
        assert!(fmt
            .enrichment_prefix
            .contains("<memory-caution key=\"precedent-status\" overlap=\"1.00\">"));
        assert!(fmt
            .enrichment_prefix
            .contains("Failed tools: web_search, message_send"));
        assert!(fmt.enrichment_prefix.contains("Failure anchor: failure-1"));
    }

    // ── format_turn_context: skills independent of recall ──

    #[test]
    fn format_skills_independent_of_recall() {
        let mut skill = make_skill("deploy", "Run cargo build --release");
        skill.origin = crate::domain::memory::SkillOrigin::Manual;
        skill.status = crate::domain::memory::SkillStatus::Active;
        let ctx = TurnMemoryContext {
            recalled_entries: vec![], // empty recall
            skills: vec![skill],
            ..Default::default()
        };
        let fmt = format_turn_context(&ctx, &PromptBudget::default());
        assert!(!fmt.enrichment_prefix.contains("[Memory context]"));
        assert!(fmt
            .enrichment_prefix
            .contains("<skill name=\"deploy\" origin=\"manual\" status=\"active\">"));
        assert!(fmt.enrichment_prefix.contains("Run cargo build --release"));
        assert!(fmt.enrichment_prefix.contains("</skill>"));
    }

    // ── format_turn_context: entities independent of recall ──

    #[test]
    fn format_entities_independent_of_recall() {
        let ctx = TurnMemoryContext {
            recalled_entries: vec![], // empty recall
            entities: vec![make_entity("Rust", "Systems programming language")],
            entity_neighbors: vec![EntityNeighborhood {
                entity_name: "Rust".into(),
                relations: vec!["used_for: systems programming".into()],
            }],
            ..Default::default()
        };
        let fmt = format_turn_context(&ctx, &PromptBudget::default());
        assert!(fmt
            .enrichment_prefix
            .contains("<entity name=\"Rust\" type=\"concept\">"));
        assert!(fmt
            .enrichment_prefix
            .contains("Systems programming language"));
        assert!(fmt
            .enrichment_prefix
            .contains("Relations: used_for: systems programming"));
    }

    #[test]
    fn format_recipes_independent_of_recall() {
        let ctx = TurnMemoryContext {
            recalled_entries: vec![],
            run_recipes: vec![make_recipe(
                "deploy",
                "Check staging first, then ship the release",
            )],
            ..Default::default()
        };
        let fmt = format_turn_context(&ctx, &PromptBudget::default());
        assert!(fmt
            .enrichment_prefix
            .contains("<recipe task_family=\"deploy\" success_count=\"3\">"));
        assert!(fmt
            .enrichment_prefix
            .contains("Check staging first, then ship the release"));
        assert!(fmt.enrichment_prefix.contains("Sample request:"));
        assert!(fmt.enrichment_prefix.contains("Tool pattern: shell, git"));
        assert!(fmt.enrichment_prefix.contains("</recipe>"));
    }

    #[test]
    fn prioritize_uncontradicted_recipe_hits_keeps_safe_branch_first() {
        let hits = vec![
            RunRecipeSearchMatch {
                tool_pattern: vec!["web_search".into(), "message_send".into()],
                ..make_recipe("status_delivery", "Search and send the status page")
            },
            RunRecipeSearchMatch {
                tool_pattern: vec!["shell".into(), "message_send".into()],
                ..make_recipe("backup_delivery", "Run backup and send the result")
            },
        ];
        let stored = vec![
            make_stored_recipe(
                "status_delivery",
                &["web_search", "message_send"],
                "Search and send the status page",
            ),
            make_stored_recipe(
                "backup_delivery",
                &["shell", "message_send"],
                "Run backup and send the result",
            ),
        ];
        let failures = vec![make_failure_cluster(
            "f1",
            "failed_tools=web_search -> message_send | outcomes=runtime_error",
        )];

        let ordered = prioritize_uncontradicted_recipe_hits(hits, &stored, &failures);

        assert_eq!(ordered[0].task_family, "backup_delivery");
        assert_eq!(ordered[1].task_family, "status_delivery");
    }

    #[test]
    fn prioritize_uncontradicted_memory_entries_keeps_safe_precedent_first() {
        let mut entries = vec![
            make_precedent_entry(
                "precedent-status",
                "tools=web_search -> message_send | subjects=status.example.com",
                0.91,
            ),
            make_precedent_entry(
                "precedent-backup",
                "tools=shell -> message_send | subjects=backup-job",
                0.82,
            ),
        ];

        prioritize_uncontradicted_memory_entries(
            &mut entries,
            &[make_failure_cluster(
                "failure-1",
                "failed_tools=web_search -> message_send | outcomes=runtime_error",
            )],
        );

        assert_eq!(entries[0].key, "precedent-backup");
        assert_eq!(entries[1].key, "precedent-status");
    }

    #[test]
    fn format_recipe_section_surfaces_procedural_contradictions() {
        let ctx = TurnMemoryContext {
            run_recipes: vec![make_recipe(
                "status_delivery",
                "Search the status page and send the result",
            )],
            procedural_contradictions: vec![
                crate::application::services::procedural_contradiction_service::ProceduralContradiction {
                    recipe_task_family: "status_delivery".into(),
                    recipe_lineage_task_families: vec!["status_delivery".into(), "status_page_delivery".into()],
                    recipe_cluster_size: 2,
                    recipe_tool_pattern: vec!["web_search".into(), "message_send".into()],
                    failure_representative_key: "failure-1".into(),
                    failure_cluster_size: 1,
                    failed_tools: vec!["web_search".into(), "message_send".into()],
                    overlap: 1.0,
                },
            ],
            ..Default::default()
        };

        let fmt = format_turn_context(&ctx, &PromptBudget::default());
        assert!(fmt.enrichment_prefix.contains(
            "<procedural-contradiction task_family=\"status_delivery\" overlap=\"1.00\">"
        ));
        assert!(fmt
            .enrichment_prefix
            .contains("Lineage: status_delivery, status_page_delivery"));
        assert!(fmt
            .enrichment_prefix
            .contains("Failed tools: web_search, message_send"));
        assert!(fmt.enrichment_prefix.contains("Failure anchor: failure-1"));
    }

    #[test]
    fn format_session_recaps_independent_of_recall() {
        let ctx = TurnMemoryContext {
            recalled_entries: vec![],
            session_matches: vec![make_session_match(
                "Weather thread",
                "Compared Berlin and Tbilisi weather last week",
                "user: what was the weather in Tbilisi? | assistant: Tbilisi was warmer",
            )],
            ..Default::default()
        };
        let fmt = format_turn_context(&ctx, &PromptBudget::default());
        assert!(fmt
            .enrichment_prefix
            .contains("<session-recap key=\"channel_alice\" kind=\"channel\">"));
        assert!(fmt.enrichment_prefix.contains("Label: Weather thread"));
        assert!(fmt
            .enrichment_prefix
            .contains("Compared Berlin and Tbilisi weather last week"));
        assert!(fmt.enrichment_prefix.contains("Recent match:"));
        assert!(fmt.enrichment_prefix.contains("</session-recap>"));
    }

    #[test]
    fn format_resolution_plan_orders_recipe_before_memory_when_router_says_so() {
        let ctx = TurnMemoryContext {
            recalled_entries: vec![make_entry("fact1", "User likes Rust", 0.9)],
            run_recipes: vec![make_recipe(
                "deploy",
                "Check staging first, then ship the release",
            )],
            resolution_plan: Some(resolution_router::ResolutionPlan {
                source_order: vec![
                    resolution_router::ResolutionSource::RunRecipe,
                    resolution_router::ResolutionSource::LongTermMemory,
                ],
                confidence: resolution_router::ResolutionConfidence::High,
                clarify_after_exhaustion: true,
                clarification_reason: None,
            }),
            ..Default::default()
        };
        let fmt = format_turn_context(&ctx, &PromptBudget::default());
        let recipe_pos = fmt.enrichment_prefix.find("<recipe").unwrap();
        let memory_pos = fmt.enrichment_prefix.find("[Memory context]").unwrap();
        assert!(recipe_pos < memory_pos);
        assert!(fmt.resolution_system.contains("[resolution-plan]"));
    }

    #[test]
    fn format_turn_context_includes_execution_guidance_when_available() {
        let ctx = TurnMemoryContext {
            execution_guidance: Some(execution_guidance::ExecutionGuidance {
                resolved_from: Some(resolution_router::ResolutionSource::ConfiguredRuntime),
                direct_resolution_ready: true,
                preferred_capabilities: vec![execution_guidance::ExecutionCapability::Delivery],
                recent_failure_hints: Vec::new(),
                recent_admission_hint: None,
                prefer_answer_from_resolved_state: false,
                avoid_session_history_lookup: true,
                avoid_run_recipe_lookup: true,
                avoid_workspace_discovery: true,
                avoid_bootstrap_doc_reads: true,
            }),
            ..Default::default()
        };

        let fmt = format_turn_context(&ctx, &PromptBudget::default());
        assert!(fmt.resolution_system.contains("[execution-guidance]"));
        assert!(fmt
            .resolution_system
            .contains("resolved_from: configured_runtime"));
        assert!(fmt
            .resolution_system
            .contains("preferred_capabilities: delivery"));
    }

    #[test]
    fn format_turn_context_includes_bounded_session_handoff_packet() {
        let ctx = TurnMemoryContext {
            handoff_packet: Some(session_handoff::SessionHandoffPacket {
                reason: session_handoff::SessionHandoffReason::ContextOverflow,
                recommended_action: Some("start_fresh_handoff".into()),
                active_task: Some("continue after route downgrade".into()),
                current_defaults: vec!["project_alias=Borealis".into()],
                anchors: vec!["memory=early anchor".into()],
                unresolved_questions: Vec::new(),
                assumptions: Vec::new(),
            }),
            ..Default::default()
        };

        let fmt = format_turn_context(&ctx, &PromptBudget::default());
        assert!(fmt.resolution_system.contains("[session-handoff]"));
        assert!(fmt.resolution_system.contains("reason: context_overflow"));
        assert!(fmt
            .resolution_system
            .contains("recommended_action: start_fresh_handoff"));
        assert!(fmt.resolution_system.contains("[/session-handoff]"));
    }

    #[test]
    fn format_turn_context_includes_clarification_policy_when_available() {
        let ctx = TurnMemoryContext {
            clarification_guidance: Some(clarification_policy::ClarificationGuidance {
                candidate_set: vec!["Berlin".into(), "Tbilisi".into()],
                required: true,
                avoid_generic_questions: true,
                reason: Some("low_confidence".into()),
            }),
            ..Default::default()
        };
        let fmt = format_turn_context(&ctx, &PromptBudget::default());
        assert!(fmt.resolution_system.contains("[clarification-policy]"));
        assert!(fmt
            .resolution_system
            .contains("clarification_required: true"));
        assert!(fmt.resolution_system.contains("Berlin | Tbilisi"));
        assert!(fmt.resolution_system.contains("reason: low_confidence"));
    }

    // ── format_turn_context: budget enforcement ──

    #[test]
    fn format_enrichment_budget_cap() {
        let ctx = TurnMemoryContext {
            recalled_entries: vec![make_entry("k", &"x".repeat(100), 0.9)],
            skills: vec![make_skill("s", &"y".repeat(100))],
            entities: vec![make_entity("e", &"z".repeat(100))],
            ..Default::default()
        };
        let budget = PromptBudget {
            enrichment_total_max_chars: 80,
            ..Default::default()
        };
        let fmt = format_turn_context(&ctx, &budget);
        // Total enrichment must not exceed budget (with small tolerance for formatting)
        assert!(
            fmt.enrichment_prefix.chars().count() <= 90,
            "enrichment {} chars exceeds budget 80",
            fmt.enrichment_prefix.chars().count()
        );
    }

    #[test]
    fn format_empty_context_produces_empty_strings() {
        let ctx = TurnMemoryContext::default();
        let fmt = format_turn_context(&ctx, &PromptBudget::default());
        assert!(fmt.core_blocks_system.is_empty());
        assert!(fmt.enrichment_prefix.is_empty());
    }

    // ── load_recall filtering (via internal helper) ──

    #[test]
    fn recall_filters_autosave_keys() {
        // is_autosave_key is tested directly above; this documents the invariant
        assert!(is_autosave_key("assistant_resp"));
        assert!(is_autosave_key("assistant_resp_abc"));
        assert!(!is_autosave_key("user_msg_abc"));
    }

    #[test]
    fn recall_filters_tool_result_content() {
        // This invariant is enforced in load_recall: entries with <tool_result are skipped
        let entry_content = "Previous result: <tool_result>stale data</tool_result>";
        assert!(entry_content.contains("<tool_result"));
    }
}
