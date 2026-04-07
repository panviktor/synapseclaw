//! Cheap duplicate compaction for learned episodic categories.
//!
//! This keeps precedent and failure-pattern memories from growing through
//! obvious near-duplicates. The worker keeps the newer representative and
//! removes older duplicates.

use crate::application::services::failure_similarity_service;
use crate::application::services::precedent_similarity_service;
use crate::application::services::procedural_cluster_service::ProceduralCluster;
use crate::domain::memory::{MemoryCategory, MemoryEntry, MemoryError};
use crate::ports::memory::UnifiedMemoryPort;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct DuplicateCompactionThresholds {
    pub shortlist_limit: usize,
    pub duplicate_threshold: f64,
}

impl DuplicateCompactionThresholds {
    pub fn precedent_defaults() -> Self {
        Self {
            shortlist_limit: 6,
            duplicate_threshold: 0.95,
        }
    }

    pub fn failure_pattern_defaults() -> Self {
        Self {
            shortlist_limit: 6,
            duplicate_threshold: 0.96,
        }
    }
}

pub async fn compact_near_duplicates(
    mem: &dyn UnifiedMemoryPort,
    agent_id: &str,
    category: MemoryCategory,
    limit: usize,
    thresholds: &DuplicateCompactionThresholds,
) -> Result<usize, MemoryError> {
    compact_near_duplicates_with_failures(mem, agent_id, category, limit, thresholds, &[]).await
}

pub async fn compact_near_duplicates_with_failures(
    mem: &dyn UnifiedMemoryPort,
    agent_id: &str,
    category: MemoryCategory,
    limit: usize,
    thresholds: &DuplicateCompactionThresholds,
    failure_clusters: &[ProceduralCluster],
) -> Result<usize, MemoryError> {
    let entries = mem.list_scoped(Some(&category), None, limit, false).await?;
    if entries.len() < 2 {
        return Ok(0);
    }

    let similarity_lookup = fetch_category_shortlists(
        mem,
        agent_id,
        &entries,
        &category,
        thresholds.shortlist_limit,
    )
    .await?;

    let duplicate_keys = plan_duplicate_removals(
        &entries,
        &similarity_lookup,
        &category,
        thresholds.duplicate_threshold,
        failure_clusters,
    );
    let mut removed = 0;
    for key in duplicate_keys {
        if mem.forget(&key, &agent_id.to_string()).await? {
            removed += 1;
        }
    }
    Ok(removed)
}

async fn fetch_category_shortlists(
    mem: &dyn UnifiedMemoryPort,
    agent_id: &str,
    entries: &[MemoryEntry],
    category: &MemoryCategory,
    limit: usize,
) -> Result<HashMap<String, Vec<MemoryEntry>>, MemoryError> {
    Ok(mem
        .similar_episodes_for_entries(entries, agent_id, category, limit, false)
        .await?
        .into_iter()
        .map(|(key, values)| {
            let shortlist = values
                .into_iter()
                .map(|result| {
                    let mut entry = result.entry;
                    entry.score = Some(result.score as f64);
                    entry
                })
                .collect::<Vec<_>>();
            (key, shortlist)
        })
        .collect())
}

pub fn plan_duplicate_removals(
    entries: &[MemoryEntry],
    similarity_lookup: &HashMap<String, Vec<MemoryEntry>>,
    category: &MemoryCategory,
    duplicate_threshold: f64,
    failure_clusters: &[ProceduralCluster],
) -> Vec<String> {
    let mut ordered = entries.to_vec();
    ordered.sort_by(|left, right| {
        parse_timestamp(&right.timestamp)
            .cmp(&parse_timestamp(&left.timestamp))
            .then_with(|| left.key.cmp(&right.key))
    });

    let mut kept = HashSet::new();
    let mut removed = HashSet::new();

    for entry in &ordered {
        if removed.contains(&entry.key) {
            continue;
        }

        let Some(shortlist) = similarity_lookup.get(&entry.key) else {
            kept.insert(entry.key.clone());
            continue;
        };

        for similar in shortlist {
            if similar.key == entry.key || removed.contains(&similar.key) {
                continue;
            }
            if similar.score.unwrap_or(0.0) < duplicate_threshold {
                continue;
            }
            if distinct_procedural_shape(category, &entry.content, &similar.content) {
                continue;
            }
            if distinct_contradiction_state(
                category,
                &entry.content,
                &similar.content,
                failure_clusters,
            ) {
                continue;
            }
            if kept.contains(&similar.key) {
                removed.insert(entry.key.clone());
                break;
            }
            removed.insert(similar.key.clone());
        }

        if !removed.contains(&entry.key) {
            kept.insert(entry.key.clone());
        }
    }

    let mut keys = removed.into_iter().collect::<Vec<_>>();
    keys.sort();
    keys
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

fn distinct_contradiction_state(
    category: &MemoryCategory,
    left: &str,
    right: &str,
    failure_clusters: &[ProceduralCluster],
) -> bool {
    match category {
        MemoryCategory::Custom(name) if name == "precedent" && !failure_clusters.is_empty() => {
            let left_contradicted =
                precedent_similarity_service::precedent_is_contradicted_by_failures(
                    left,
                    failure_clusters,
                    0.75,
                );
            let right_contradicted =
                precedent_similarity_service::precedent_is_contradicted_by_failures(
                    right,
                    failure_clusters,
                    0.75,
                );
            left_contradicted != right_contradicted
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

    fn entry(key: &str, timestamp: &str) -> MemoryEntry {
        MemoryEntry {
            id: key.into(),
            key: key.into(),
            content: format!("entry:{key}"),
            category: MemoryCategory::Custom("precedent".into()),
            timestamp: timestamp.into(),
            session_id: None,
            score: None,
        }
    }

    fn similar(key: &str, score: f64) -> MemoryEntry {
        MemoryEntry {
            id: key.into(),
            key: key.into(),
            content: format!("entry:{key}"),
            category: MemoryCategory::Custom("precedent".into()),
            timestamp: "2026-01-01T00:00:00Z".into(),
            session_id: None,
            score: Some(score),
        }
    }

    fn failure_cluster(summary: &str) -> ProceduralCluster {
        ProceduralCluster {
            representative: MemoryEntry {
                id: "f1".into(),
                key: "f1".into(),
                content: summary.into(),
                category: MemoryCategory::Custom("failure_pattern".into()),
                timestamp: "2026-01-01T00:00:00Z".into(),
                session_id: None,
                score: None,
            },
            member_keys: vec!["f1".into()],
        }
    }

    #[test]
    fn removes_older_duplicate_when_newer_entry_kept() {
        let entries = vec![
            entry("newer", "2026-02-01T00:00:00Z"),
            entry("older", "2026-01-01T00:00:00Z"),
        ];
        let similarity_lookup = HashMap::from([
            (
                "newer".into(),
                vec![similar("newer", 1.0), similar("older", 0.98)],
            ),
            (
                "older".into(),
                vec![similar("older", 1.0), similar("newer", 0.98)],
            ),
        ]);

        let removed = plan_duplicate_removals(
            &entries,
            &similarity_lookup,
            &MemoryCategory::Custom("precedent".into()),
            0.95,
            &[],
        );
        assert_eq!(removed, vec!["older".to_string()]);
    }

    #[test]
    fn removes_current_entry_if_duplicate_of_kept_newer_entry() {
        let entries = vec![
            entry("first", "2026-03-01T00:00:00Z"),
            entry("second", "2026-02-01T00:00:00Z"),
            entry("third", "2026-01-01T00:00:00Z"),
        ];
        let similarity_lookup = HashMap::from([
            (
                "first".into(),
                vec![similar("first", 1.0), similar("second", 0.97)],
            ),
            (
                "second".into(),
                vec![similar("second", 1.0), similar("first", 0.97)],
            ),
            ("third".into(), vec![similar("third", 1.0)]),
        ]);

        let removed = plan_duplicate_removals(
            &entries,
            &similarity_lookup,
            &MemoryCategory::Custom("precedent".into()),
            0.95,
            &[],
        );
        assert_eq!(removed, vec!["second".to_string()]);
    }

    #[test]
    fn keeps_high_similarity_precedents_with_distinct_tool_shape() {
        let entries = vec![
            MemoryEntry {
                id: "newer".into(),
                key: "newer".into(),
                content:
                    "tools=web_search -> message_send | subjects=status.example.com | facets=send"
                        .into(),
                category: MemoryCategory::Custom("precedent".into()),
                timestamp: "2026-03-01T00:00:00Z".into(),
                session_id: None,
                score: None,
            },
            MemoryEntry {
                id: "older".into(),
                key: "older".into(),
                content: "tools=shell -> message_send | subjects=status.example.com | facets=send"
                    .into(),
                category: MemoryCategory::Custom("precedent".into()),
                timestamp: "2026-02-01T00:00:00Z".into(),
                session_id: None,
                score: None,
            },
        ];
        let similarity_lookup = HashMap::from([
            (
                "newer".into(),
                vec![
                    similar("newer", 1.0),
                    MemoryEntry {
                        id: "older".into(),
                        key: "older".into(),
                        content: "tools=shell -> message_send | subjects=status.example.com | facets=send".into(),
                        category: MemoryCategory::Custom("precedent".into()),
                        timestamp: "2026-02-01T00:00:00Z".into(),
                        session_id: None,
                        score: Some(0.98),
                    },
                ],
            ),
            (
                "older".into(),
                vec![
                    MemoryEntry {
                        id: "older".into(),
                        key: "older".into(),
                        content: "tools=shell -> message_send | subjects=status.example.com | facets=send".into(),
                        category: MemoryCategory::Custom("precedent".into()),
                        timestamp: "2026-02-01T00:00:00Z".into(),
                        session_id: None,
                        score: Some(1.0),
                    },
                    MemoryEntry {
                        id: "newer".into(),
                        key: "newer".into(),
                        content: "tools=web_search -> message_send | subjects=status.example.com | facets=send".into(),
                        category: MemoryCategory::Custom("precedent".into()),
                        timestamp: "2026-03-01T00:00:00Z".into(),
                        session_id: None,
                        score: Some(0.98),
                    },
                ],
            ),
        ]);

        let removed = plan_duplicate_removals(
            &entries,
            &similarity_lookup,
            &MemoryCategory::Custom("precedent".into()),
            0.95,
            &[],
        );
        assert!(removed.is_empty());
    }

    #[test]
    fn keeps_high_similarity_failure_patterns_with_distinct_failed_tools() {
        let entries = vec![
            MemoryEntry {
                id: "newer".into(),
                key: "newer".into(),
                content: "failed_tools=web_fetch -> message_send | outcomes=runtime_error | subjects=status.example.com".into(),
                category: MemoryCategory::Custom("failure_pattern".into()),
                timestamp: "2026-03-01T00:00:00Z".into(),
                session_id: None,
                score: None,
            },
            MemoryEntry {
                id: "older".into(),
                key: "older".into(),
                content: "failed_tools=shell -> message_send | outcomes=runtime_error | subjects=status.example.com".into(),
                category: MemoryCategory::Custom("failure_pattern".into()),
                timestamp: "2026-02-01T00:00:00Z".into(),
                session_id: None,
                score: None,
            },
        ];
        let similarity_lookup = HashMap::from([
            (
                "newer".into(),
                vec![
                    MemoryEntry {
                        id: "newer".into(),
                        key: "newer".into(),
                        content: "failed_tools=web_fetch -> message_send | outcomes=runtime_error | subjects=status.example.com".into(),
                        category: MemoryCategory::Custom("failure_pattern".into()),
                        timestamp: "2026-03-01T00:00:00Z".into(),
                        session_id: None,
                        score: Some(1.0),
                    },
                    MemoryEntry {
                        id: "older".into(),
                        key: "older".into(),
                        content: "failed_tools=shell -> message_send | outcomes=runtime_error | subjects=status.example.com".into(),
                        category: MemoryCategory::Custom("failure_pattern".into()),
                        timestamp: "2026-02-01T00:00:00Z".into(),
                        session_id: None,
                        score: Some(0.98),
                    },
                ],
            ),
            (
                "older".into(),
                vec![
                    MemoryEntry {
                        id: "older".into(),
                        key: "older".into(),
                        content: "failed_tools=shell -> message_send | outcomes=runtime_error | subjects=status.example.com".into(),
                        category: MemoryCategory::Custom("failure_pattern".into()),
                        timestamp: "2026-02-01T00:00:00Z".into(),
                        session_id: None,
                        score: Some(1.0),
                    },
                    MemoryEntry {
                        id: "newer".into(),
                        key: "newer".into(),
                        content: "failed_tools=web_fetch -> message_send | outcomes=runtime_error | subjects=status.example.com".into(),
                        category: MemoryCategory::Custom("failure_pattern".into()),
                        timestamp: "2026-03-01T00:00:00Z".into(),
                        session_id: None,
                        score: Some(0.98),
                    },
                ],
            ),
        ]);

        let removed = plan_duplicate_removals(
            &entries,
            &similarity_lookup,
            &MemoryCategory::Custom("failure_pattern".into()),
            0.95,
            &[],
        );
        assert!(removed.is_empty());
    }

    #[test]
    fn keeps_precedent_duplicates_when_contradiction_state_differs() {
        let supported = MemoryEntry {
            id: "supported".into(),
            key: "supported".into(),
            content: "tools=web_search -> message_send -> file_write | subjects=status.example.com"
                .into(),
            category: MemoryCategory::Custom("precedent".into()),
            timestamp: "2026-03-01T00:00:00Z".into(),
            session_id: None,
            score: None,
        };
        let contradicted = MemoryEntry {
            id: "contradicted".into(),
            key: "contradicted".into(),
            content: "tools=web_search -> message_send | subjects=status.example.com".into(),
            category: MemoryCategory::Custom("precedent".into()),
            timestamp: "2026-02-01T00:00:00Z".into(),
            session_id: None,
            score: None,
        };
        let similarity_lookup = HashMap::from([
            (
                "supported".into(),
                vec![
                    MemoryEntry {
                        score: Some(1.0),
                        ..supported.clone()
                    },
                    MemoryEntry {
                        score: Some(0.98),
                        ..contradicted.clone()
                    },
                ],
            ),
            (
                "contradicted".into(),
                vec![
                    MemoryEntry {
                        score: Some(1.0),
                        ..contradicted.clone()
                    },
                    MemoryEntry {
                        score: Some(0.98),
                        ..supported.clone()
                    },
                ],
            ),
        ]);

        let removed = plan_duplicate_removals(
            &[supported, contradicted],
            &similarity_lookup,
            &MemoryCategory::Custom("precedent".into()),
            0.95,
            &[failure_cluster(
                "failed_tools=web_search -> message_send | outcomes=runtime_error",
            )],
        );
        assert!(removed.is_empty());
    }
}
