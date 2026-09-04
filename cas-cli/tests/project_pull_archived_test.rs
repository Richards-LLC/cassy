//! Regression coverage for cas-c9a4: project pulls must reconcile archived
//! local entries instead of trying to insert the same primary key again.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cas::cloud::{
    CloudConfig, CloudSyncer, CloudSyncerConfig, SyncQueue, get_project_canonical_id,
};
use cas::store::{
    SqliteStore, Store, StoreError, open_commit_link_store, open_event_store,
    open_file_change_store, open_prompt_store, open_rule_store_local, open_skill_store_local,
    open_spec_store, open_store_local, open_task_store_local,
};
use cas::types::{Entry, EntryType, Scope};
use chrono::{DateTime, Duration, Utc};
use tempfile::TempDir;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct RaceStore {
    inner: SqliteStore,
    race_id: String,
    concurrent_entry: Entry,
    raced: AtomicBool,
}

impl RaceStore {
    fn new(inner: SqliteStore, concurrent_entry: Entry) -> Self {
        Self {
            inner,
            race_id: concurrent_entry.id.clone(),
            concurrent_entry,
            raced: AtomicBool::new(false),
        }
    }
}

impl Store for RaceStore {
    fn init(&self) -> cas::store::Result<()> {
        self.inner.init()
    }

    fn generate_id(&self) -> cas::store::Result<String> {
        self.inner.generate_id()
    }

    fn add(&self, entry: &Entry) -> cas::store::Result<()> {
        if entry.id == self.race_id && !self.raced.swap(true, Ordering::SeqCst) {
            self.inner.add(&self.concurrent_entry)?;
            return Err(StoreError::EntryExists(entry.id.clone()));
        }
        self.inner.add(entry)
    }

    fn get(&self, id: &str) -> cas::store::Result<Entry> {
        self.inner.get(id)
    }

    fn get_archived(&self, id: &str) -> cas::store::Result<Entry> {
        self.inner.get_archived(id)
    }

    fn update(&self, entry: &Entry) -> cas::store::Result<()> {
        self.inner.update(entry)
    }

    fn delete(&self, id: &str) -> cas::store::Result<()> {
        self.inner.delete(id)
    }

    fn list(&self) -> cas::store::Result<Vec<Entry>> {
        self.inner.list()
    }

    fn recent(&self, n: usize) -> cas::store::Result<Vec<Entry>> {
        self.inner.recent(n)
    }

    fn recent_timestamp(&self, entry: &Entry) -> cas::store::Result<DateTime<Utc>> {
        self.inner.recent_timestamp(entry)
    }

    fn archive(&self, id: &str) -> cas::store::Result<()> {
        self.inner.archive(id)
    }

    fn unarchive(&self, id: &str) -> cas::store::Result<()> {
        self.inner.unarchive(id)
    }

    fn list_archived(&self) -> cas::store::Result<Vec<Entry>> {
        self.inner.list_archived()
    }

    fn list_by_branch(&self, branch: &str) -> cas::store::Result<Vec<Entry>> {
        self.inner.list_by_branch(branch)
    }

    fn list_pending(&self, limit: usize) -> cas::store::Result<Vec<Entry>> {
        self.inner.list_pending(limit)
    }

    fn mark_extracted(&self, id: &str) -> cas::store::Result<()> {
        self.inner.mark_extracted(id)
    }

    fn list_pinned(&self) -> cas::store::Result<Vec<Entry>> {
        self.inner.list_pinned()
    }

    fn list_helpful(&self, limit: usize) -> cas::store::Result<Vec<Entry>> {
        self.inner.list_helpful(limit)
    }

    fn list_by_session(&self, session_id: &str) -> cas::store::Result<Vec<Entry>> {
        self.inner.list_by_session(session_id)
    }

    fn list_unreviewed_learnings(&self, limit: usize) -> cas::store::Result<Vec<Entry>> {
        self.inner.list_unreviewed_learnings(limit)
    }

    fn mark_reviewed(&self, id: &str) -> cas::store::Result<()> {
        self.inner.mark_reviewed(id)
    }

    fn list_pending_index(&self, limit: usize) -> cas::store::Result<Vec<Entry>> {
        self.inner.list_pending_index(limit)
    }

    fn mark_indexed(&self, id: &str) -> cas::store::Result<()> {
        self.inner.mark_indexed(id)
    }

    fn mark_indexed_batch(&self, ids: &[&str]) -> cas::store::Result<()> {
        self.inner.mark_indexed_batch(ids)
    }

    fn mark_index_pending_batch(&self, ids: &[&str]) -> cas::store::Result<()> {
        self.inner.mark_index_pending_batch(ids)
    }

    fn cas_dir(&self) -> &Path {
        self.inner.cas_dir()
    }

    fn close(&self) -> cas::store::Result<()> {
        self.inner.close()
    }
}

struct PullHarness {
    _tmp: TempDir,
    syncer: CloudSyncer,
    store: Arc<dyn Store>,
    task_store: Arc<dyn cas::store::TaskStore>,
    rule_store: Arc<dyn cas::store::RuleStore>,
    skill_store: Arc<dyn cas::store::SkillStore>,
    spec_store: Arc<dyn cas::store::SpecStore>,
    event_store: Arc<dyn cas::store::EventStore>,
    prompt_store: Arc<dyn cas::store::PromptStore>,
    file_change_store: Arc<dyn cas::store::FileChangeStore>,
    commit_link_store: Arc<dyn cas::store::CommitLinkStore>,
}

impl PullHarness {
    fn new(endpoint: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        // Pin the scratch root: the ephemeral-project guard refuses an unpinned
        // root under the temp directory, and a TempDir is exactly that.
        std::fs::write(tmp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
        let root = tmp.path();
        let store = open_store_local(root).unwrap();
        Self::from_store(tmp, endpoint, store)
    }

    fn with_duplicate_race(endpoint: &str, concurrent_entry: Entry) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        // Pin the scratch root: the ephemeral-project guard refuses an unpinned
        // root under the temp directory, and a TempDir is exactly that.
        std::fs::write(tmp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
        let root = tmp.path();
        let inner = SqliteStore::open(root).unwrap();
        inner.init().unwrap();
        let store: Arc<dyn Store> = Arc::new(RaceStore::new(inner, concurrent_entry));
        Self::from_store(tmp, endpoint, store)
    }

    fn from_store(tmp: TempDir, endpoint: &str, store: Arc<dyn Store>) -> Self {
        let root = tmp.path();
        let task_store = open_task_store_local(root).unwrap();
        let rule_store = open_rule_store_local(root).unwrap();
        let skill_store = open_skill_store_local(root).unwrap();
        let spec_store = open_spec_store(root).unwrap();
        let event_store = open_event_store(root).unwrap();
        let prompt_store = open_prompt_store(root).unwrap();
        let file_change_store = open_file_change_store(root).unwrap();
        let commit_link_store = open_commit_link_store(root).unwrap();
        let queue = Arc::new(SyncQueue::open(root).unwrap());
        queue.init().unwrap();

        let syncer = CloudSyncer::new(
            queue,
            CloudConfig {
                endpoint: endpoint.to_string(),
                token: Some("test-token".to_string()),
                ..Default::default()
            },
            CloudSyncerConfig::default(),
        );

        Self {
            _tmp: tmp,
            syncer,
            store,
            task_store,
            rule_store,
            skill_store,
            spec_store,
            event_store,
            prompt_store,
            file_change_store,
            commit_link_store,
        }
    }

    fn pull(&self) -> Result<cas::cloud::SyncResult, cas::error::CasError> {
        self.syncer.pull(
            self.store.as_ref(),
            self.task_store.as_ref(),
            self.rule_store.as_ref(),
            self.skill_store.as_ref(),
            self.spec_store.as_ref(),
            self.event_store.as_ref(),
            self.prompt_store.as_ref(),
            self.file_change_store.as_ref(),
            self.commit_link_store.as_ref(),
        )
    }
}

fn entry(id: &str, content: &str, created: DateTime<Utc>) -> Entry {
    Entry {
        id: id.to_string(),
        scope: Scope::Project,
        entry_type: EntryType::Context,
        content: content.to_string(),
        created,
        ..Default::default()
    }
}

fn remote_entry(entry: Entry, project_id: &str, updated_at: DateTime<Utc>) -> serde_json::Value {
    let mut raw = serde_json::to_value(entry).unwrap();
    raw["project_id"] = serde_json::json!(project_id);
    raw["project_canonical_id"] = serde_json::json!(project_id);
    raw["updated_at"] = serde_json::json!(updated_at.to_rfc3339());
    raw
}

async fn mount_pull(server: &MockServer, project_id: &str, entries: Vec<serde_json::Value>) {
    Mock::given(method("GET"))
        .and(path("/api/sync/pull"))
        .and(query_param("project_id", project_id))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": entries,
            "tasks": [],
            "rules": [],
            "skills": [],
            "specs": [],
            "events": [],
            "prompts": [],
            "file_changes": [],
            "commit_links": [],
            "pulled_at": "2026-09-01T00:00:00Z",
        })))
        .expect(1)
        .mount(server)
        .await;
}

fn assert_no_pull_errors(result: &cas::cloud::SyncResult) {
    assert!(
        result.errors.is_empty(),
        "unexpected pull errors: {:?}",
        result.errors
    );
}

#[tokio::test]
async fn archived_local_newer_remote_updates_and_unarchives() {
    let server = MockServer::start().await;
    let project_id = get_project_canonical_id().expect("test runs inside a CAS project");
    let now = Utc::now();
    mount_pull(
        &server,
        &project_id,
        vec![remote_entry(
            entry("project-archived-newer", "remote", now - Duration::hours(2)),
            &project_id,
            now + Duration::hours(1),
        )],
    )
    .await;

    let harness = PullHarness::new(&server.uri());
    harness
        .store
        .add(&entry(
            "project-archived-newer",
            "local archived",
            now - Duration::hours(3),
        ))
        .unwrap();
    harness.store.archive("project-archived-newer").unwrap();

    let store = Arc::clone(&harness.store);
    let result = tokio::task::spawn_blocking(move || harness.pull())
        .await
        .unwrap()
        .unwrap();

    assert_no_pull_errors(&result);
    assert_eq!(result.pulled_entries, 1);
    assert_eq!(
        store.get("project-archived-newer").unwrap().content,
        "remote"
    );
    assert!(store.get_archived("project-archived-newer").is_err());
}

#[tokio::test]
async fn archived_local_older_remote_skips_and_stays_archived() {
    let server = MockServer::start().await;
    let project_id = get_project_canonical_id().expect("test runs inside a CAS project");
    let now = Utc::now();
    mount_pull(
        &server,
        &project_id,
        vec![remote_entry(
            entry("project-archived-older", "remote", now - Duration::hours(2)),
            &project_id,
            now - Duration::hours(1),
        )],
    )
    .await;

    let harness = PullHarness::new(&server.uri());
    harness
        .store
        .add(&entry(
            "project-archived-older",
            "local archived",
            now - Duration::hours(3),
        ))
        .unwrap();
    harness.store.archive("project-archived-older").unwrap();

    let store = Arc::clone(&harness.store);
    let result = tokio::task::spawn_blocking(move || harness.pull())
        .await
        .unwrap()
        .unwrap();

    assert_no_pull_errors(&result);
    assert_eq!(result.pulled_entries, 0);
    assert_eq!(result.conflicts_resolved, 1);
    assert_eq!(
        store
            .get_archived("project-archived-older")
            .unwrap()
            .content,
        "local archived"
    );
}

#[tokio::test]
async fn active_local_lww_behavior_remains_unchanged() {
    let server = MockServer::start().await;
    let project_id = get_project_canonical_id().expect("test runs inside a CAS project");
    let now = Utc::now();
    mount_pull(
        &server,
        &project_id,
        vec![
            remote_entry(
                entry(
                    "project-active-newer",
                    "remote newer",
                    now - Duration::hours(2),
                ),
                &project_id,
                now + Duration::hours(1),
            ),
            remote_entry(
                entry(
                    "project-active-older",
                    "remote older",
                    now - Duration::hours(2),
                ),
                &project_id,
                now - Duration::hours(1),
            ),
        ],
    )
    .await;

    let harness = PullHarness::new(&server.uri());
    harness
        .store
        .add(&entry(
            "project-active-newer",
            "local",
            now - Duration::hours(3),
        ))
        .unwrap();
    harness
        .store
        .add(&entry(
            "project-active-older",
            "local",
            now - Duration::hours(3),
        ))
        .unwrap();

    let store = Arc::clone(&harness.store);
    let result = tokio::task::spawn_blocking(move || harness.pull())
        .await
        .unwrap()
        .unwrap();

    assert_no_pull_errors(&result);
    assert_eq!(result.pulled_entries, 1);
    assert_eq!(result.conflicts_resolved_local, 1);
    assert_eq!(result.conflicts_resolved_remote, 1);
    assert_eq!(result.conflicts_resolved, 2);
    assert_eq!(
        store.get("project-active-newer").unwrap().content,
        "remote newer"
    );
    assert_eq!(store.get("project-active-older").unwrap().content, "local");
}

#[tokio::test]
async fn duplicate_race_reconciles_through_lww() {
    let server = MockServer::start().await;
    let project_id = get_project_canonical_id().expect("test runs inside a CAS project");
    let now = Utc::now();
    mount_pull(
        &server,
        &project_id,
        vec![remote_entry(
            entry("project-duplicate-race", "remote", now - Duration::hours(2)),
            &project_id,
            now + Duration::hours(1),
        )],
    )
    .await;

    let harness = PullHarness::with_duplicate_race(
        &server.uri(),
        entry(
            "project-duplicate-race",
            "concurrent local",
            now - Duration::hours(3),
        ),
    );

    let store = Arc::clone(&harness.store);
    let result = tokio::task::spawn_blocking(move || harness.pull())
        .await
        .unwrap()
        .unwrap();

    assert_no_pull_errors(&result);
    assert_eq!(result.pulled_entries, 1);
    assert_eq!(
        store.get("project-duplicate-race").unwrap().content,
        "remote"
    );
}

#[tokio::test]
async fn unrelated_store_error_is_reported() {
    let server = MockServer::start().await;
    let project_id = get_project_canonical_id().expect("test runs inside a CAS project");
    let now = Utc::now();
    mount_pull(
        &server,
        &project_id,
        vec![remote_entry(
            entry("project-store-error", "remote", now),
            &project_id,
            now + Duration::hours(1),
        )],
    )
    .await;

    let harness = PullHarness::new(&server.uri());
    drop_entries_table(harness._tmp.path());

    let result = tokio::task::spawn_blocking(move || harness.pull())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(result.pulled_entries, 0);
    assert_eq!(result.errors.len(), 1);
    assert!(
        result.errors[0].contains("Entry error:"),
        "{}",
        result.errors[0]
    );
    assert!(
        result.errors[0].contains("no such table"),
        "{}",
        result.errors[0]
    );
}

fn drop_entries_table(root: &std::path::Path) {
    let connection = rusqlite::Connection::open(root.join("cas.db")).unwrap();
    connection.execute_batch("DROP TABLE entries").unwrap();
}
