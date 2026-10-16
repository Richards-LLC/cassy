//! Real-store regressions for cas-1a2a: a committed archive must not fail its
//! post-write read, and archived IDs remain addressable by memory operations.

use std::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::ErrorCode;
use serde_json::{Value, json};
use tempfile::TempDir;

use super::CasService;
use crate::cloud::SyncQueue;
use crate::config::Config;
use crate::hybrid_search::DocType;
use crate::mcp::server::CasCore;
use crate::store::{MarkdownStore, SqliteStore, Store, SyncingEntryStore};
use crate::types::{BeliefType, Entry, MemoryTier};

fn service(markdown: bool) -> (TempDir, CasService, Arc<dyn Store>, Arc<SyncQueue>) {
    let dir = TempDir::new().unwrap();
    let inner: Arc<dyn Store> = if markdown {
        Arc::new(MarkdownStore::open(dir.path()).unwrap())
    } else {
        Arc::new(SqliteStore::open(dir.path()).unwrap())
    };
    inner.init().unwrap();
    let queue = Arc::new(SyncQueue::open(dir.path()).unwrap());
    queue.init().unwrap();
    let store: Arc<dyn Store> = Arc::new(SyncingEntryStore::new(inner, queue.clone()));
    let core = CasCore::with_daemon(dir.path().to_path_buf(), None, None);
    assert!(core.cached_store.set(store.clone()).is_ok());
    assert!(core.cached_config.set(Config::default()).is_ok());
    #[cfg(feature = "mcp-proxy")]
    let service = CasService::new(core, None);
    #[cfg(not(feature = "mcp-proxy"))]
    let service = CasService::new(core);
    (dir, service, store, queue)
}

async fn memory(service: &CasService, request: Value) -> String {
    let result = service
        .memory(Parameters(serde_json::from_value(request).unwrap()))
        .await
        .expect("memory action must succeed");
    assert_eq!(result.is_error, Some(false));
    result
        .content
        .iter()
        .filter_map(|content| content.as_text().map(|text| text.text.as_str()))
        .collect::<Vec<_>>()
        .join("\n")
}

async fn archive_lifecycle(markdown: bool) {
    let (_dir, service, store, queue) = service(markdown);
    let entry = Entry::new("archive-lifecycle".into(), "Lifecycle fixture".into());
    store.add(&entry).unwrap();
    queue.clear().unwrap();
    let search = service.inner.open_search_index().unwrap();
    search.index_entry(&entry).unwrap();

    assert!(
        memory(&service, json!({"action": "get", "id": entry.id}))
            .await
            .contains("Archived: false")
    );
    assert_eq!(
        memory(&service, json!({"action": "archive", "id": entry.id})).await,
        "Archived entry: archive-lifecycle"
    );
    assert!(store.get_archived(&entry.id).unwrap().archived);
    assert!(store.list().unwrap().is_empty());
    assert_eq!(search.count_documents(DocType::Entry).unwrap(), 0);
    let pending = queue.pending(10, 5).unwrap();
    assert_eq!(pending.len(), 1);
    let payload: Entry = serde_json::from_str(pending[0].payload.as_ref().unwrap()).unwrap();
    assert!(payload.archived);
    queue.clear().unwrap();

    // Simulate the stale index left by the old post-write failure. Retrying
    // archive must repair it while accurately reporting the existing state.
    search.index_entry(&entry).unwrap();
    assert_eq!(search.count_documents(DocType::Entry).unwrap(), 1);

    assert_eq!(
        memory(&service, json!({"action": "archive", "id": entry.id})).await,
        "Entry already archived: archive-lifecycle"
    );
    assert!(queue.pending(10, 5).unwrap().is_empty());
    assert_eq!(search.count_documents(DocType::Entry).unwrap(), 0);
    assert!(
        memory(&service, json!({"action": "get", "id": entry.id}))
            .await
            .contains("Archived: true")
    );
    assert!(
        store.get(&entry.id).is_err(),
        "get must not silently restore the entry"
    );

    assert_eq!(
        memory(&service, json!({"action": "unarchive", "id": entry.id})).await,
        "Restored entry: archive-lifecycle"
    );
    assert!(!store.get(&entry.id).unwrap().archived);
    assert_eq!(search.count_documents(DocType::Entry).unwrap(), 1);
    assert_eq!(
        memory(&service, json!({"action": "unarchive", "id": entry.id})).await,
        "Entry already active: archive-lifecycle"
    );
}

#[tokio::test]
async fn memory_archive_lifecycle_resolves_archived_ids_sqlite() {
    archive_lifecycle(false).await;
}

#[tokio::test]
async fn memory_archive_lifecycle_resolves_archived_ids_markdown() {
    archive_lifecycle(true).await;
}

#[tokio::test]
async fn memory_id_mutations_resolve_archived_entries_without_restoring_them() {
    let (_dir, service, store, queue) = service(false);
    let mut entry = Entry::new("archived-mutations".into(), "Archived fixture".into());
    entry.belief_type = BeliefType::Opinion;
    entry.confidence = 0.5;
    store.add(&entry).unwrap();
    store.archive(&entry.id).unwrap();

    memory(
        &service,
        json!({"action": "set_tier", "id": entry.id, "tier": "archive"}),
    )
    .await;
    assert_eq!(
        store.get_archived(&entry.id).unwrap().memory_tier,
        MemoryTier::Archive
    );
    queue.clear().unwrap();
    memory(&service, json!({"action": "get", "id": entry.id})).await;
    assert_eq!(
        store.get_archived(&entry.id).unwrap().memory_tier,
        MemoryTier::Archive
    );
    assert!(
        queue.pending(10, 5).unwrap().is_empty(),
        "get only updates local access metadata"
    );
    for action in [
        "helpful",
        "harmful",
        "mark_reviewed",
        "update",
        "opinion_reinforce",
        "opinion_weaken",
        "opinion_contradict",
    ] {
        memory(
            &service,
            json!({"action": action, "id": entry.id,
                               "content": "Edited archived fixture"}),
        )
        .await;
        assert!(store.get_archived(&entry.id).unwrap().archived, "{action}");
    }
    let persisted = store.get_archived(&entry.id).unwrap();
    assert_eq!(persisted.content, "Edited archived fixture");
    assert_eq!(persisted.helpful_count, 2);
    assert_eq!(persisted.harmful_count, 4);
    assert!(persisted.last_reviewed.is_some());
    assert!((persisted.confidence - 0.4).abs() < 0.001);
    memory(&service, json!({"action": "delete", "id": entry.id})).await;
    assert!(store.get_archived(&entry.id).is_err());
}

#[tokio::test]
async fn memory_contradiction_archives_live_opinion_without_false_failure() {
    let (_dir, service, store, queue) = service(false);
    let mut entry = Entry::new("weak-opinion".into(), "Low-confidence opinion".into());
    entry.belief_type = BeliefType::Opinion;
    entry.confidence = 0.15;
    store.add(&entry).unwrap();
    queue.clear().unwrap();
    assert!(
        memory(
            &service,
            json!({"action": "opinion_contradict", "id": entry.id,
                                   "content": "Contradicting evidence"})
        )
        .await
        .contains("archived due to very low confidence")
    );
    assert!(store.get_archived(&entry.id).unwrap().archived);
    let pending = queue.pending(10, 5).unwrap();
    assert_eq!(pending.len(), 1);
    let payload: Entry = serde_json::from_str(pending[0].payload.as_ref().unwrap()).unwrap();
    assert!(payload.archived);
}

#[tokio::test]
async fn memory_missing_id_remains_a_lookup_error() {
    let (_dir, service, _store, _queue) = service(false);
    for action in [
        "get",
        "archive",
        "unarchive",
        "set_tier",
        "update",
        "delete",
        "helpful",
        "harmful",
        "mark_reviewed",
        "opinion_reinforce",
        "opinion_weaken",
        "opinion_contradict",
    ] {
        let error = service
            .memory(Parameters(
                serde_json::from_value(json!({
                    "action": action, "id": "missing", "tier": "cold", "content": "Evidence",
                }))
                .unwrap(),
            ))
            .await
            .expect_err("unknown IDs must remain errors");
        assert_eq!(error.code, ErrorCode::INVALID_PARAMS, "{action}");
        assert!(
            error.message.contains("entry not found"),
            "{action}: {error}"
        );
    }
}

#[tokio::test]
async fn cas_e29c_unknown_birth_is_labelled_and_sorts_after_known_dates() {
    let _env = crate::test_support::TestEnvGuard::temp_home();
    let (_dir, service, store, _queue) = service(false);
    let unknown: Entry = serde_json::from_value(json!({
        "id": "unknown-fold0123456789ab", "content": "undated memory"
    }))
    .unwrap();
    let mut known = Entry::new("known-date".into(), "dated memory".into());
    known.created = "2026-10-01T00:00:00Z".parse().unwrap();
    store.add(&unknown).unwrap();
    store.add(&known).unwrap();
    for sort in ["created", "updated"] {
        for order in ["asc", "desc"] {
            let listed = memory(
                &service,
                json!({"action":"list", "sort":sort, "sort_order":order}),
            )
            .await;
            assert!(!listed.contains("0001-01-01"), "{listed}");
            assert!(listed.contains("unknown"), "{listed}");
            assert!(
                listed.find("[known-date]").unwrap() < listed.find("[unknown-fold").unwrap(),
                "{listed}"
            );
        }
    }
    let shown = memory(&service, json!({"action":"get", "id":unknown.id})).await;
    assert!(shown.contains("Created: unknown"), "{shown}");
    assert!(!shown.contains("0001-01-01"), "{shown}");
}

#[tokio::test]
async fn cas_e29c_search_provenance_never_exposes_unknown_birth_as_a_date() {
    let _env = crate::test_support::TestEnvGuard::temp_home();
    let (_dir, service, store, _queue) = service(false);
    let entry: Entry = serde_json::from_value(json!({
        "id":"unknown-fold0123456789ab", "content":"unique sentinel probe"
    }))
    .unwrap();
    store.add(&entry).unwrap();
    service
        .inner
        .open_search_index()
        .unwrap()
        .index_entry(&entry)
        .unwrap();
    let result = service.search(Parameters(serde_json::from_value(json!({
        "action":"search", "query":"sentinel probe", "doc_type":"entry", "provenance_version":1
    })).unwrap())).await.unwrap();
    let text = result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.as_str()))
        .collect::<Vec<_>>()
        .join("\n");
    let payload: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(payload["hits"][0]["id"], entry.id);
    assert!(
        payload["hits"][0]["provenance"]["freshness"]["created_at"].is_null(),
        "{text}"
    );
    assert!(!text.contains("0001-01-01"), "{text}");
}
