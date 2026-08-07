//! Running one query case against one retrieval surface.

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

fn to_hits(rows: &[MemoryRow]) -> Vec<Hit> {
    rows.iter()
        .enumerate()
        .map(|(rank, row)| Hit {
            rank,
            id: row.id.clone(),
            fp: fingerprint(&row.content),
            label: label_for(row.title.as_deref(), &row.content),
            entry_type: row.entry_type.clone(),
            tier: row.memory_tier.clone(),
        })
        .collect()
}

/// Execute one case.
pub fn run_case(
    db: &ReadOnlyMemoryDb,
    index: &IndexHandle,
    case: &QueryCase,
    set: &QuerySet,
) -> Result<QueryResult, ParityError> {
    let limit = set.limit_for(case);
    let arg = case.query.trim();

    let (status, hits) = match case.channel {
        Channel::Recent => (ChannelStatus::Ok, to_hits(&db.recent(limit)?)),
        Channel::List => (ChannelStatus::Ok, to_hits(&db.list(limit)?)),
        Channel::Pinned => (ChannelStatus::Ok, to_hits(&db.pinned(limit)?)),
        Channel::Helpful => (ChannelStatus::Ok, to_hits(&db.helpful(limit)?)),
        Channel::ByType => (ChannelStatus::Ok, to_hits(&db.by_type(arg, limit)?)),
        Channel::ByTier => (ChannelStatus::Ok, to_hits(&db.by_tier(arg, limit)?)),
        Channel::ByTag => (ChannelStatus::Ok, to_hits(&db.by_tag(arg, limit)?)),
        Channel::Search => match index {
            IndexHandle::Unavailable(reason) => (
                ChannelStatus::Unavailable {
                    reason: reason.clone(),
                },
                Vec::new(),
            ),
            IndexHandle::Ready(index) => (ChannelStatus::Ok, run_search(db, index, arg, limit)?),
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
    let mut hits = Vec::new();
    for id in &ids {
        let Some(row) = rows.get(id) else { continue };
        hits.push(Hit {
            rank: hits.len(),
            id: row.id.clone(),
            fp: fingerprint(&row.content),
            label: label_for(row.title.as_deref(), &row.content),
            entry_type: row.entry_type.clone(),
            tier: row.memory_tier.clone(),
        });
    }
    Ok(hits)
}
