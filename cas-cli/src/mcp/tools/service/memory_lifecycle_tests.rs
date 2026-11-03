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

#[tokio::test]
async fn superseding_a_memory_expires_it_and_removes_it_from_session_start() {
    use cas_core::hooks::{
        config::DefaultHooksConfig,
        context::{ContextStores, build_context_with_stores},
        types::HookInput,
    };
    for markdown in [false, true] {
        let (_dir, service, store, _queue) = service(markdown);
        let mut old = Entry::new(
            "stale-freeze".into(),
            "A superseded stakeholder gate".into(),
        );
        old.memory_tier = MemoryTier::InContext;
        store.add(&old).unwrap();
        let render = || {
            let stores = ContextStores {
                project_store: Some(store.as_ref()),
                ..ContextStores::empty()
            };
            build_context_with_stores(
                &HookInput::default(),
                &stores,
                &DefaultHooksConfig::new(),
                10,
                None,
                "mcp__cs__",
            )
            .unwrap()
            .0
        };
        assert!(render().contains("stale-freeze"));
        memory(&service, json!({"action": "update", "id": old.id, "tags": "superseded,superseded-by:current-policy"})).await;
        let stored = store.get(&old.id).unwrap();
        assert!(
            stored
                .valid_until
                .is_some_and(|until| until <= chrono::Utc::now()),
            "supersession must persist expiry"
        );
        assert!(!render().contains("stale-freeze"));
        assert_eq!(stored.content, old.content, "history stays available by ID");
    }
}

fn policy_service() -> (TempDir, CasService) {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join(".cas");
    std::fs::create_dir(&root).unwrap();
    let core = CasCore::with_daemon(root, None, None);
    assert!(core.cached_config.set(Config::default()).is_ok());
    core.open_rule_store().unwrap().init().unwrap();
    #[cfg(feature = "mcp-proxy")]
    let service = CasService::new(core, None);
    #[cfg(not(feature = "mcp-proxy"))]
    let service = CasService::new(core);
    (dir, service)
}

#[tokio::test]
async fn rule_drift_reports_generated_and_peer_status_without_mutating_policy() {
    use cas_types::{Rule, RuleStatus};
    let (dir, service) = policy_service();
    let rules = service.inner.open_rule_store().unwrap();
    for id in ["rule-177", "rule-183", "rule-192"] {
        let mut rule = Rule::new(id.into(), "Current stakeholder instruction".into());
        rule.status = RuleStatus::Proven;
        rule.helpful_count = 3;
        rules.add(&rule).unwrap();
    }
    service.inner.sync_rules().unwrap();
    let generated = dir.path().join(".claude/rules/cas/rule-177.md");
    std::fs::write(&generated, "Stale broad gate").unwrap();
    let peer = json!({
        "machine": "soundwave", "project": "policy-fixture", "captured_at": "2026-11-03T12:00:00Z",
        "cloud": {"auto_sync": true, "pull_on_start": true, "interval_secs": 60},
        "rules": rules.list().unwrap().into_iter().map(|mut r| {
            if r.id != "rule-177" { r.status = RuleStatus::Retired; }
            r
        }).collect::<Vec<_>>()
    });
    // Bind fixture project identity for cross-machine snapshots.
    std::fs::write(
        dir.path().join(".cas/config.toml"),
        "[project]\ncanonical_id='policy-fixture'\n",
    )
    .unwrap();
    let result = service
        .rule(Parameters(
            serde_json::from_value(json!({
                "action": "drift", "peer_snapshot": peer.to_string()
            }))
            .unwrap(),
        ))
        .await
        .expect("report-only drift action must be available");
    assert_eq!(result.is_error, Some(false));
    let report = result
        .content
        .iter()
        .filter_map(|c| c.as_text())
        .map(|c| c.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    for finding in [
        "rule-177",
        "generated",
        "rule-183",
        "rule-192",
        "proven",
        "retired",
        "cloud",
        "soundwave",
    ] {
        assert!(report.contains(finding), "missing {finding}: {report}");
    }
    // pin: A report-only check must preserve the stale generated file byte for byte.
    assert_eq!(
        std::fs::read_to_string(&generated).unwrap(),
        "Stale broad gate"
    );
    assert!(
        rules
            .list()
            .unwrap()
            .iter()
            .all(|r| r.status == RuleStatus::Proven)
    );
    assert!(
        result.structured_content.is_some(),
        "the local snapshot must be exportable for a peer check"
    );
    let snapshot = &result.structured_content.as_ref().unwrap()["snapshot"];
    assert!(
        snapshot["cloud"].is_null(),
        "missing [cloud] must stay visible in the export"
    );
    let mut wrong_project = peer;
    wrong_project["project"] = json!("another-project");
    let rejected = service
        .rule(Parameters(
            serde_json::from_value(
                json!({"action": "drift", "peer_snapshot": wrong_project.to_string()}),
            )
            .unwrap(),
        ))
        .await;
    assert!(
        rejected.is_err(),
        "a different project's policy must not be compared as a peer"
    );
    std::fs::write(dir.path().join(".cas/config.toml"), "[project]\ncanonical_id='policy-fixture'\n[cloud]\nauto_sync=true\npull_on_start=true\ninterval_secs=60\n").unwrap();
    let fresh = service
        .rule(Parameters(
            serde_json::from_value(json!({"action": "drift"})).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(
        fresh.structured_content.unwrap()["snapshot"]["cloud"]["auto_sync"],
        true,
        "the check must read current source config even when the MCP server cached older settings"
    );
}

#[tokio::test]
async fn stakeholder_rule_requires_and_preserves_verbatim_scope_and_review_date() {
    let (_dir, service) = policy_service();
    let words = "  Review PROMPT changes via PR.\nDon't ask me to approve wording in Slack.  ";
    let scope = "Prompt changes in this project; staging delivery remains immediate.";
    for extra in [
        json!({}),
        json!({"stakeholder_quote": words}),
        json!({"stakeholder_quote": words, "stakeholder_scope": scope}),
    ] {
        let mut request = json!({"action": "create", "content": "HARD RULE: Prompt changes via PR", "stakeholder": "Ben"});
        request
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let result = service
            .rule(Parameters(serde_json::from_value(request).unwrap()))
            .await;
        assert!(
            result.is_err() || result.as_ref().is_ok_and(|r| r.is_error == Some(true)),
            "incomplete attribution was accepted"
        );
    }
    assert!(
        service
            .inner
            .open_rule_store()
            .unwrap()
            .list()
            .unwrap()
            .is_empty()
    );
    let result = service.rule(Parameters(serde_json::from_value(json!({
        "action": "create", "content": "HARD RULE: Prompt changes via PR", "stakeholder": "Ben",
        "stakeholder_quote": words, "stakeholder_scope": scope, "review_after": "2100-01-01T00:00:00Z"
    })).unwrap())).await.unwrap();
    assert_ne!(result.is_error, Some(true));
    let rule = service
        .inner
        .open_rule_store()
        .unwrap()
        .list()
        .unwrap()
        .pop()
        .unwrap();
    let saved = serde_json::to_value(&rule).unwrap();
    assert_eq!(saved["stakeholder"]["name"], "Ben");
    assert_eq!(
        saved["stakeholder"]["quote"], words,
        "never paraphrase or trim the quote"
    );
    assert_eq!(saved["stakeholder"]["scope"], scope);
    assert!(rule.review_after.is_some());
    let shown = service
        .rule(Parameters(
            serde_json::from_value(json!({"action": "show", "id": rule.id})).unwrap(),
        ))
        .await
        .unwrap();
    let text = shown
        .content
        .iter()
        .filter_map(|c| c.as_text())
        .map(|c| c.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains(words) && text.contains(scope), "{text}");
    let changed_quote = "Keep shipping to staging; review prompts via PR.";
    let updated = service
        .rule(Parameters(
            serde_json::from_value(
                json!({"action": "update", "id": rule.id, "stakeholder_quote": changed_quote}),
            )
            .unwrap(),
        ))
        .await
        .unwrap();
    assert_ne!(updated.is_error, Some(true));
    let updated = service
        .inner
        .open_rule_store()
        .unwrap()
        .get(&rule.id)
        .unwrap();
    assert_eq!(updated.stakeholder.as_ref().unwrap().quote, changed_quote);
    assert_eq!(updated.stakeholder.as_ref().unwrap().scope, scope);
    let cleared = service.rule(Parameters(serde_json::from_value(json!({
        "action": "update", "id": rule.id, "stakeholder": "", "change_note": "Explicitly clear attribution"
    })).unwrap())).await.expect("explicit empty stakeholder must clear attribution");
    assert_ne!(cleared.is_error, Some(true));
    let store = service.inner.open_rule_store().unwrap();
    assert!(store.get(&rule.id).unwrap().stakeholder.is_none());
    let latest: cas_types::Rule = serde_json::from_str(&store.list_versions(&rule.id).unwrap()[0].snapshot_json).unwrap();
    assert!(latest.stakeholder.is_none(), "clear and resulting history must commit together");
}

#[tokio::test]
async fn stakeholder_constraint_memories_require_an_explicit_expiry() {
    let (_dir, service, store, _queue) = service(false);
    let mut request = json!({"action": "remember", "content": "Freeze SMS edits for this review", "tags": "stakeholder:Ben,constraint", "bypass_overlap": true, "personal": true});
    let result = service
        .memory(Parameters(serde_json::from_value(request.clone()).unwrap()))
        .await;
    assert!(
        result.is_err() || result.as_ref().is_ok_and(|r| r.is_error == Some(true)),
        "stakeholder constraints must not become permanent memories"
    );
    assert!(store.list().unwrap().is_empty());
    request["valid_until"] = json!("2100-01-01T00:00:00Z");
    memory(&service, request).await;
    let entry = store.list().unwrap().pop().unwrap();
    assert!(entry.valid_until.is_some());
    memory(
        &service,
        json!({"action": "update", "id": entry.id, "valid_until": "2101-01-01T00:00:00Z"}),
    )
    .await;
    memory(&service, json!({"action": "update", "id": entry.id, "tags": "stakeholder:Ben,constraint,superseded-by:new-policy"})).await;
    assert!(store.get(&entry.id).unwrap().is_expired());
}
