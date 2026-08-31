//! Detection and one-time repair for the legacy daemon Tantivy root.

use std::path::{Component, Path};

use tantivy::collector::DocSetCollector;
use tantivy::query::AllQuery;
use tantivy::schema::Value;

use crate::error::{CasError, Result};
use crate::hybrid_search::{BackgroundIndexer, IndexingConfig};
use crate::store::Store;

/// Read-only description of an index accidentally written directly below
/// `<cas_root>/index` by BackgroundIndexer before cas-bc42.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyIndexState {
    pub documents: usize,
    pub entry_ids: Vec<String>,
}

/// Outcome of migrating a legacy root into the canonical Tantivy index.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegacyRepairResult {
    pub legacy_documents: usize,
    pub requeued_entries: usize,
    pub indexed_entries: usize,
}

/// Inspect the legacy root without creating an index when none exists.
pub fn inspect_legacy_index(cas_dir: &Path) -> Result<Option<LegacyIndexState>> {
    let legacy_dir = cas_dir.join("index");
    if !legacy_dir.join("meta.json").is_file() {
        return Ok(None);
    }

    let index = tantivy::Index::open_in_dir(&legacy_dir)?;
    let schema = index.schema();
    let id_field = schema
        .get_field("id")
        .map_err(|_| CasError::Other("legacy Tantivy index has no `id` field".to_string()))?;
    let doc_type_field = schema
        .get_field("doc_type")
        .map_err(|_| CasError::Other("legacy Tantivy index has no `doc_type` field".to_string()))?;
    let reader = index.reader()?;
    let searcher = reader.searcher();
    let addresses = searcher.search(&AllQuery, &DocSetCollector)?;
    let documents = addresses.len();
    let mut entry_ids = Vec::new();
    for address in addresses {
        let document: tantivy::TantivyDocument = searcher.doc(address)?;
        let doc_type = document
            .get_first(doc_type_field)
            .and_then(|value| value.as_str());
        if doc_type != Some("entry") {
            continue;
        }
        if let Some(id) = document
            .get_first(id_field)
            .and_then(|value| value.as_str())
        {
            entry_ids.push(id.to_string());
        }
    }
    entry_ids.sort();
    entry_ids.dedup();

    Ok(Some(LegacyIndexState {
        documents,
        entry_ids,
    }))
}

/// Re-queue every legacy entry, retire only Tantivy-managed root files, and
/// drain the pending queue into the canonical index.
pub fn repair_legacy_index(
    cas_dir: &Path,
    store: &dyn Store,
) -> Result<Option<LegacyRepairResult>> {
    let Some(state) = inspect_legacy_index(cas_dir)? else {
        return Ok(None);
    };

    let ids: Vec<&str> = state.entry_ids.iter().map(String::as_str).collect();
    store.mark_index_pending_batch(&ids)?;
    retire_legacy_index_files(cas_dir)?;

    let indexer = BackgroundIndexer::open(cas_dir)?;
    let indexed = indexer.process_pending(
        store,
        &IndexingConfig {
            batch_size: 256,
            max_per_run: usize::MAX,
        },
    )?;
    if !indexed.errors.is_empty() {
        return Err(CasError::Other(format!(
            "legacy index repair left {} indexing error(s): {}",
            indexed.errors.len(),
            indexed
                .errors
                .iter()
                .take(3)
                .map(|(id, error)| format!("{id}: {error}"))
                .collect::<Vec<_>>()
                .join("; ")
        )));
    }

    Ok(Some(LegacyRepairResult {
        legacy_documents: state.documents,
        requeued_entries: state.entry_ids.len(),
        indexed_entries: indexed.indexed,
    }))
}

fn retire_legacy_index_files(cas_dir: &Path) -> Result<()> {
    let legacy_dir = cas_dir.join("index");
    let managed_path = legacy_dir.join(".managed.json");
    let managed: Vec<String> = serde_json::from_slice(&std::fs::read(&managed_path)?)?;

    for filename in managed {
        let path = Path::new(&filename);
        if path.components().count() != 1
            || !matches!(path.components().next(), Some(Component::Normal(_)))
        {
            return Err(CasError::Other(format!(
                "refusing unsafe legacy Tantivy managed path `{filename}`"
            )));
        }
        remove_file_if_present(&legacy_dir.join(path))?;
    }
    for metadata in [
        "meta.json",
        ".managed.json",
        ".tantivy-meta.lock",
        ".tantivy-writer.lock",
    ] {
        remove_file_if_present(&legacy_dir.join(metadata))?;
    }
    Ok(())
}

fn remove_file_if_present(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hybrid_search::{
        DocType, HybridSearch, HybridSearchOptions, SearchIndex, SearchOptions,
    };
    use crate::store::open_store;
    use crate::types::Entry;

    #[test]
    fn repair_requeues_legacy_entries_into_the_canonical_index_and_preserves_siblings() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cas_root = temp.path().join(".cas");
        std::fs::create_dir_all(&cas_root).expect("create .cas");
        let store = open_store(&cas_root).expect("store");
        let entry = Entry::new(
            "legacy-daemon-only".to_string(),
            "legacyquasar background repair target".to_string(),
        );
        store.add(&entry).expect("add entry");

        {
            let legacy = SearchIndex::open(&cas_root.join("index")).expect("legacy index");
            legacy.index_entry(&entry).expect("index legacy entry");
        }
        store
            .mark_indexed(&entry.id)
            .expect("mark incorrectly indexed");
        std::fs::create_dir_all(cas_root.join("index/code")).expect("code dir");
        std::fs::write(cas_root.join("index/code/keep"), b"code-index-sibling")
            .expect("code marker");

        let state = inspect_legacy_index(&cas_root)
            .expect("inspect")
            .expect("legacy state");
        assert_eq!(state.documents, 1);
        assert_eq!(state.entry_ids, vec![entry.id.clone()]);

        let repair = repair_legacy_index(&cas_root, store.as_ref())
            .expect("repair")
            .expect("repair result");
        assert_eq!(repair.legacy_documents, 1);
        assert_eq!(repair.requeued_entries, 1);
        assert_eq!(repair.indexed_entries, 1);
        assert!(
            inspect_legacy_index(&cas_root)
                .expect("clean inspect")
                .is_none()
        );
        assert_eq!(
            std::fs::read(cas_root.join("index/code/keep")).expect("preserved marker"),
            b"code-index-sibling"
        );
        assert!(store.list_pending_index(10).expect("pending").is_empty());

        let search = HybridSearch::open(&cas_root).expect("canonical reader");
        let results = search
            .search(
                &HybridSearchOptions {
                    base: SearchOptions {
                        query: "legacyquasar".to_string(),
                        doc_types: vec![DocType::Entry],
                        ..Default::default()
                    },
                    enable_temporal: false,
                    enable_graph: false,
                    ..Default::default()
                },
                &store.list().expect("entries"),
            )
            .expect("search canonical index");
        assert_eq!(
            results.first().map(|result| result.id.as_str()),
            Some(entry.id.as_str())
        );
    }
}
