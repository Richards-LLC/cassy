//! Running one query case against one retrieval surface.

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::hybrid_search::{DocType, EXPECTED_FIELD_COUNT, SearchIndex, SearchOptions};

use super::store_ro::{MemoryRow, ReadOnlyMemoryDb};
use super::{Channel, Hit, ParityError, QueryCase, QueryResult, QuerySet, fingerprint, label_for};

/// Whether a channel could actually be probed.
///
/// An unavailable channel is *not* silently treated as "no hits": that would
/// let a missing index masquerade as an empty-but-healthy result at capture
/// time, and as a total wipeout at replay time. It is recorded explicitly and
/// the diff treats a became-unavailable channel as a regression.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ChannelStatus {
    Ok,
    Unavailable { reason: String },
}

impl ChannelStatus {
    pub fn is_ok(&self) -> bool {
        matches!(self, ChannelStatus::Ok)
    }
}

/// An opened search index, or the reason there isn't one.
pub enum IndexHandle {
    Ready(Box<SearchIndex>),
    Unavailable(String),
}

/// Open the memory search index **only if doing so cannot modify it**.
///
/// [`SearchIndex::open`] creates the directory when absent and, on a
/// field-count mismatch, `remove_dir_all`s the index and rebuilds it. Either
/// would make the harness a writer to the very artifact it is measuring, so
/// both preconditions are checked first; on failure the search channel reports
/// [`ChannelStatus::Unavailable`] rather than triggering a rebuild.
pub fn open_index_if_compatible(index_dir: &Path) -> IndexHandle {
    if !index_dir.join("meta.json").exists() {
        return IndexHandle::Unavailable(format!("no search index at {}", index_dir.display()));
    }
    // Count the on-disk schema's fields ourselves rather than letting
    // SearchIndex::open discover the mismatch and react by deleting the index.
    let existing = match tantivy::Index::open_in_dir(index_dir) {
        Ok(index) => index,
        Err(e) => {
            return IndexHandle::Unavailable(format!(
                "search index at {} is unreadable: {e}",
                index_dir.display()
            ));
        }
    };
    let field_count = existing.schema().fields().count();
    drop(existing);
    if field_count != EXPECTED_FIELD_COUNT {
        return IndexHandle::Unavailable(format!(
            "search index at {} has {field_count} schema fields, expected \
             {EXPECTED_FIELD_COUNT}; the harness will not rebuild it (that would be a write)",
            index_dir.display()
        ));
    }
    match SearchIndex::open(index_dir) {
        Ok(index) => IndexHandle::Ready(Box::new(index)),
        Err(e) => IndexHandle::Unavailable(format!(
            "cannot open search index at {}: {e}",
            index_dir.display()
        )),
    }
}

/// Turn rows into ranked hits, dropping excluded fixture content.
///
/// Ranks are assigned **after** exclusion so they stay contiguous. This
/// matters: if fixtures kept their slots, the recorded rank of a real memory
/// would depend on how many fixtures happened to sit above it, and any
/// migration that removed fixtures would look like a mass rank *improvement*
/// while a migration that added them would look like a mass regression.
fn to_hits(rows: &[MemoryRow], excluded: &HashSet<String>) -> Vec<Hit> {
    let mut hits = Vec::new();
    for row in rows {
        let fp = fingerprint(&row.content);
        if excluded.contains(&fp) {
            continue;
        }
        hits.push(Hit {
            rank: hits.len(),
            id: row.id.clone(),
            fp,
            label: label_for(row.title.as_deref(), &row.content),
            entry_type: row.entry_type.clone(),
            tier: row.memory_tier.clone(),
        });
    }
    hits
}

/// Reproduce `merge_entries` (`crates/cas-core/src/hooks/context/mod.rs:520`):
/// project entries first, then global entries whose id — with the `p-`/`g-`
/// scope prefix stripped — has not already been seen. Project wins ties.
fn merge_project_over_global(project: &[MemoryRow], global: &[MemoryRow]) -> Vec<MemoryRow> {
    fn dedup_key(id: &str) -> &str {
        id.strip_prefix("p-")
            .or_else(|| id.strip_prefix("g-"))
            .unwrap_or(id)
    }

    let mut seen: HashSet<&str> = HashSet::new();
    let mut merged: Vec<MemoryRow> = Vec::with_capacity(project.len() + global.len());
    for row in project {
        if seen.insert(dedup_key(&row.id)) {
            merged.push(row.clone());
        }
    }
    for row in global {
        if seen.insert(dedup_key(&row.id)) {
            merged.push(row.clone());
        }
    }
    merged
}

/// Everything the channels need to reach data, resolved once per run.
pub struct RunEnv<'a> {
    /// The project memory store.
    pub project: &'a ReadOnlyMemoryDb,
    /// The global memory store, when one exists on this machine. Only the
    /// `session_merge` channel reads it, mirroring `merge_entries`.
    pub global: Option<&'a ReadOnlyMemoryDb>,
    pub index: &'a IndexHandle,
    /// Fingerprints of excluded fixture content.
    pub excluded: HashSet<String>,
}

/// `store_list`'s real ceiling. The SessionStart merge reads with
/// `LIMIT 10000`, so the merge channel must too — a smaller limit would
/// baseline a truncation the production path never applies.
pub const SESSION_MERGE_LIMIT: usize = 10_000;

/// Execute one case.
pub fn run_case(
    env: &RunEnv<'_>,
    case: &QueryCase,
    set: &QuerySet,
) -> Result<QueryResult, ParityError> {
    let db = env.project;
    let limit = set.limit_for(case);
    let arg = case.query.trim();
    let ex = &env.excluded;

    let (status, hits) = match case.channel {
        Channel::Recent => (ChannelStatus::Ok, to_hits(&db.recent(limit)?, ex)),
        Channel::List => (ChannelStatus::Ok, to_hits(&db.list(limit)?, ex)),
        Channel::Pinned => (ChannelStatus::Ok, to_hits(&db.pinned(limit)?, ex)),
        Channel::Helpful => (ChannelStatus::Ok, to_hits(&db.helpful(limit)?, ex)),
        Channel::ByType => (ChannelStatus::Ok, to_hits(&db.by_type(arg, limit)?, ex)),
        Channel::ByTier => (ChannelStatus::Ok, to_hits(&db.by_tier(arg, limit)?, ex)),
        Channel::ByTag => (ChannelStatus::Ok, to_hits(&db.by_tag(arg, limit)?, ex)),
        Channel::SessionMerge => {
            let project_rows = db.list(SESSION_MERGE_LIMIT)?;
            let global_rows = match env.global {
                Some(g) => g.list(SESSION_MERGE_LIMIT)?,
                None => Vec::new(),
            };
            let merged = merge_project_over_global(&project_rows, &global_rows);
            // `limit` truncates what we *record*, not what the merge reads,
            // so the dedup outcome is the production one.
            let mut hits = to_hits(&merged, ex);
            hits.truncate(limit);
            (ChannelStatus::Ok, hits)
        }
        Channel::Search => match env.index {
            IndexHandle::Unavailable(reason) => (
                ChannelStatus::Unavailable {
                    reason: reason.clone(),
                },
                Vec::new(),
            ),
            IndexHandle::Ready(index) => {
                (ChannelStatus::Ok, run_search(db, index, arg, limit, ex)?)
            }
        },
    };

    Ok(QueryResult {
        id: case.id.clone(),
        channel: case.channel,
        query: case.query.clone(),
        status,
        hits,
    })
}

/// BM25 search restricted to memories, resolved back to store content.
fn run_search(
    db: &ReadOnlyMemoryDb,
    index: &SearchIndex,
    query: &str,
    limit: usize,
    excluded: &HashSet<String>,
) -> Result<Vec<Hit>, ParityError> {
    let opts = SearchOptions {
        query: query.to_string(),
        limit,
        doc_types: vec![DocType::Entry],
        ..Default::default()
    };
    let results = index
        .search_unified(&opts)
        .map_err(|e| ParityError::StoreUnavailable(format!("search failed: {e}")))?;

    let ids: Vec<String> = results.iter().map(|r| r.id.clone()).collect();
    let rows = db.get_many(&ids)?;

    // Hits whose id is in the index but no longer in the store are index
    // staleness, not retrievable knowledge — they are dropped and the
    // remaining hits are re-ranked contiguously so that ranks stay comparable
    // between a capture and a replay with differing amounts of staleness.
    let ordered: Vec<MemoryRow> = ids.iter().filter_map(|id| rows.get(id).cloned()).collect();
    Ok(to_hits(&ordered, excluded))
}
