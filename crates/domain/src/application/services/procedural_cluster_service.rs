//! Cheap procedural cluster planning for learned episodic memory.
//!
//! This builds inspectable clusters over recent `precedent` and
//! `failure_pattern` memories using the existing category-filtered hybrid
//! search path plus category-specific shape guards.

use crate::application::services::failure_similarity_service;
use crate::application::services::precedent_similarity_service;
use crate::domain::memory::{MemoryCategory, MemoryEntry, MemoryError, MemoryQuery};
use crate::ports::memory::UnifiedMemoryPort;
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProceduralClusterKind {
    Precedent,
    FailurePattern,
}

#[derive(Debug, Clone)]
pub struct ProceduralCluster {
    pub representative: MemoryEntry,
    pub member_keys: Vec<String>,
}

impl ProceduralCluster {
    pub fn member_count(&self) -> usize {
        self.member_keys.len()
    }
}

pub async fn plan_recent_clusters(
    mem: &dyn UnifiedMemoryPort,
    agent_id: &str,
    kind: ProceduralClusterKind,
    limit: usize,
    shortlist_limit: usize,
    similarity_threshold: f64,
) -> Result<Vec<ProceduralCluster>, MemoryError> {
    let category = cluster_category(&kind);
    let entries = mem.list(Some(&category), None, limit).await?;
    if entries.len() < 2 {
        return Ok(entries
            .into_iter()
            .map(|entry| ProceduralCluster {
                representative: entry.clone(),
                member_keys: vec![entry.key],
            })
            .collect());
    }

    let mut similarity_lookup = HashMap::new();
    for entry in &entries {
        similarity_lookup.insert(
            entry.key.clone(),
            fetch_category_shortlist(mem, agent_id, entry, &category, shortlist_limit).await?,
        );
    }

    Ok(build_clusters(
        &entries,
        &similarity_lookup,
        &category,
        similarity_threshold,
    ))
}

pub fn build_clusters(
    entries: &[MemoryEntry],
    similarity_lookup: &HashMap<String, Vec<MemoryEntry>>,
    category: &MemoryCategory,
    similarity_threshold: f64,
) -> Vec<ProceduralCluster> {
    let mut ordered = entries.to_vec();
    ordered.sort_by(|left, right| {
        parse_timestamp(&right.timestamp)
            .cmp(&parse_timestamp(&left.timestamp))
            .then_with(|| left.key.cmp(&right.key))
    });

    let entry_lookup = entries
        .iter()
        .map(|entry| (entry.key.clone(), entry.clone()))
        .collect::<HashMap<_, _>>();
    let known_keys = entry_lookup.keys().cloned().collect::<HashSet<_>>();
    let mut assigned = HashSet::new();
    let mut clusters = Vec::new();

    for entry in &ordered {
        if assigned.contains(&entry.key) {
            continue;
        }

        let mut member_keys = Vec::new();
        let mut queue = VecDeque::from([entry.key.clone()]);
        let mut queued = HashSet::from([entry.key.clone()]);

        while let Some(current_key) = queue.pop_front() {
            if assigned.contains(&current_key) {
                continue;
            }
            assigned.insert(current_key.clone());
            member_keys.push(current_key.clone());

            let Some(current_entry) = entry_lookup.get(&current_key) else {
                continue;
            };
            let Some(shortlist) = similarity_lookup.get(&current_key) else {
                continue;
            };

            for similar in shortlist {
                if similar.key == current_key || !known_keys.contains(&similar.key) {
                    continue;
                }
                if similar.score.unwrap_or(0.0) < similarity_threshold {
                    continue;
                }
                let Some(similar_entry) = entry_lookup.get(&similar.key) else {
                    continue;
                };
                if distinct_procedural_shape(
                    category,
                    &current_entry.content,
                    &similar_entry.content,
                ) {
                    continue;
                }
                if queued.insert(similar.key.clone()) {
                    queue.push_back(similar.key.clone());
                }
            }
        }

        member_keys.sort();
        clusters.push(ProceduralCluster {
            representative: entry.clone(),
            member_keys,
        });
    }

    clusters
}

async fn fetch_category_shortlist(
    mem: &dyn UnifiedMemoryPort,
    agent_id: &str,
    entry: &MemoryEntry,
    category: &MemoryCategory,
    limit: usize,
) -> Result<Vec<MemoryEntry>, MemoryError> {
    let query = MemoryQuery {
        text: entry.content.clone(),
        embedding: None,
        agent_id: agent_id.to_string(),
        categories: vec![category.clone()],
        include_shared: false,
        time_range: None,
        limit: limit.saturating_mul(2).max(limit),
    };
    let mut episodes = mem.hybrid_search(&query).await?.episodes;
    episodes.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(episodes
        .into_iter()
        .take(limit)
        .map(|result| {
            let mut entry = result.entry;
            entry.score = Some(result.score as f64);
            entry
        })
        .collect())
}

fn cluster_category(kind: &ProceduralClusterKind) -> MemoryCategory {
    match kind {
        ProceduralClusterKind::Precedent => MemoryCategory::Custom("precedent".into()),
        ProceduralClusterKind::FailurePattern => MemoryCategory::Custom("failure_pattern".into()),
    }
}

fn distinct_procedural_shape(category: &MemoryCategory, left: &str, right: &str) -> bool {
    match category {
        MemoryCategory::Custom(name) if name == "precedent" => {
            precedent_similarity_service::precedent_contents_have_distinct_shape(left, right)
        }
        MemoryCategory::Custom(name) if name == "failure_pattern" => {
            failure_similarity_service::failure_contents_have_distinct_shape(left, right)
        }
        _ => false,
    }
}

fn parse_timestamp(value: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.with_timezone(&chrono::Utc))
        .unwrap_or_else(|_| chrono::DateTime::<chrono::Utc>::UNIX_EPOCH)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: &str, category: MemoryCategory, timestamp: &str, content: &str) -> MemoryEntry {
        MemoryEntry {
            id: key.into(),
            key: key.into(),
            content: content.into(),
            category,
            timestamp: timestamp.into(),
            session_id: None,
            score: None,
        }
    }

    fn scored(key: &str, category: MemoryCategory, content: &str, score: f64) -> MemoryEntry {
        MemoryEntry {
            id: key.into(),
            key: key.into(),
            content: content.into(),
            category,
            timestamp: "2026-01-01T00:00:00Z".into(),
            session_id: None,
            score: Some(score),
        }
    }

    #[test]
    fn clusters_high_similarity_precedents_with_same_shape() {
        let category = MemoryCategory::Custom("precedent".into());
        let entries = vec![
            entry(
                "p1",
                category.clone(),
                "2026-03-01T00:00:00Z",
                "tools=web_search -> message_send | subjects=status.example.com",
            ),
            entry(
                "p2",
                category.clone(),
                "2026-02-01T00:00:00Z",
                "tools=web_search -> message_send | subjects=status2.example.com",
            ),
        ];
        let similarity_lookup = HashMap::from([
            (
                "p1".into(),
                vec![
                    scored(
                        "p1",
                        category.clone(),
                        "tools=web_search -> message_send | subjects=status.example.com",
                        1.0,
                    ),
                    scored(
                        "p2",
                        category.clone(),
                        "tools=web_search -> message_send | subjects=status2.example.com",
                        0.97,
                    ),
                ],
            ),
            (
                "p2".into(),
                vec![
                    scored(
                        "p2",
                        category.clone(),
                        "tools=web_search -> message_send | subjects=status2.example.com",
                        1.0,
                    ),
                    scored(
                        "p1",
                        category.clone(),
                        "tools=web_search -> message_send | subjects=status.example.com",
                        0.97,
                    ),
                ],
            ),
        ]);

        let clusters = build_clusters(&entries, &similarity_lookup, &category, 0.95);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].member_count(), 2);
    }

    #[test]
    fn keeps_distinct_precedent_shapes_in_separate_clusters() {
        let category = MemoryCategory::Custom("precedent".into());
        let entries = vec![
            entry(
                "p1",
                category.clone(),
                "2026-03-01T00:00:00Z",
                "tools=web_search -> message_send | subjects=status.example.com",
            ),
            entry(
                "p2",
                category.clone(),
                "2026-02-01T00:00:00Z",
                "tools=shell -> message_send | subjects=status.example.com",
            ),
        ];
        let similarity_lookup = HashMap::from([
            (
                "p1".into(),
                vec![
                    scored(
                        "p1",
                        category.clone(),
                        "tools=web_search -> message_send | subjects=status.example.com",
                        1.0,
                    ),
                    scored(
                        "p2",
                        category.clone(),
                        "tools=shell -> message_send | subjects=status.example.com",
                        0.98,
                    ),
                ],
            ),
            (
                "p2".into(),
                vec![
                    scored(
                        "p2",
                        category.clone(),
                        "tools=shell -> message_send | subjects=status.example.com",
                        1.0,
                    ),
                    scored(
                        "p1",
                        category.clone(),
                        "tools=web_search -> message_send | subjects=status.example.com",
                        0.98,
                    ),
                ],
            ),
        ]);

        let clusters = build_clusters(&entries, &similarity_lookup, &category, 0.95);
        assert_eq!(clusters.len(), 2);
    }

    #[test]
    fn clusters_failure_patterns_with_same_failed_tools() {
        let category = MemoryCategory::Custom("failure_pattern".into());
        let entries = vec![
            entry(
                "f1",
                category.clone(),
                "2026-03-01T00:00:00Z",
                "failed_tools=web_fetch | outcomes=runtime_error",
            ),
            entry(
                "f2",
                category.clone(),
                "2026-02-01T00:00:00Z",
                "failed_tools=web_fetch | outcomes=timeout",
            ),
        ];
        let similarity_lookup = HashMap::from([
            (
                "f1".into(),
                vec![
                    scored(
                        "f1",
                        category.clone(),
                        "failed_tools=web_fetch | outcomes=runtime_error",
                        1.0,
                    ),
                    scored(
                        "f2",
                        category.clone(),
                        "failed_tools=web_fetch | outcomes=timeout",
                        0.97,
                    ),
                ],
            ),
            (
                "f2".into(),
                vec![
                    scored(
                        "f2",
                        category.clone(),
                        "failed_tools=web_fetch | outcomes=timeout",
                        1.0,
                    ),
                    scored(
                        "f1",
                        category.clone(),
                        "failed_tools=web_fetch | outcomes=runtime_error",
                        0.97,
                    ),
                ],
            ),
        ]);

        let clusters = build_clusters(&entries, &similarity_lookup, &category, 0.95);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].member_count(), 2);
    }
}
