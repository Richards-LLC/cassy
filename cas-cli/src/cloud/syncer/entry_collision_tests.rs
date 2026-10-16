//! Legacy memory collisions through both real HTTP pull routes (cas-25eb1).
use super::*;
use crate::store::*;
use crate::test_support::TestEnvGuard;
use crate::types::Entry;
use chrono::{TimeZone, Utc};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PROJECT: &str = "memory-collision-project";
const TEAM: &str = "550e8400-e29b-41d4-a716-446655440000";

struct Fixture {
    _temp: tempfile::TempDir,
    root: std::path::PathBuf,
    server: MockServer,
    syncer: CloudSyncer,
    entries: Arc<dyn Store>,
    team: bool,
}

impl Fixture {
    async fn new(team: bool, local: &Entry, remote: &Entry) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = init_cas_dir(temp.path()).unwrap();
        crate::cloud::set_canonical_id_in_config_toml(&root, PROJECT).unwrap();
        let entries = open_store_local(&root).unwrap();
        entries.add(local).unwrap();
        let server = MockServer::start().await;
        let queue = Arc::new(SyncQueue::open(&root).unwrap());
        queue.init().unwrap();
        queue
            .record_revision(EntityType::Entry, &local.id, 3)
            .unwrap();
        let syncer = CloudSyncer::new_for_project(
            queue,
            CloudConfig {
                endpoint: server.uri(),
                token: Some("synthetic-token".into()),
                ..Default::default()
            },
            CloudSyncerConfig::default(),
            PROJECT.into(),
            &root,
        );
        let f = Self {
            _temp: temp,
            root,
            server,
            syncer,
            entries,
            team,
        };
        f.respond(remote).await;
        f
    }

    async fn respond(&self, remote: &Entry) {
        self.server.reset().await;
        let mut wire = serde_json::to_value(remote).unwrap();
        wire["project_id"] = serde_json::json!(PROJECT);
        wire["updated_at"] = serde_json::json!("2099-10-16T12:00:00Z");
        wire["revision"] = serde_json::json!("7");
        let endpoint = if self.team {
            format!("/api/teams/{TEAM}/sync/pull")
        } else {
            super::pull::PULL_PATH.into()
        };
        Mock::given(method("GET"))
            .and(path(endpoint))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "entries": [wire], "tasks": [], "rules": [], "skills": [],
                "pulled_at": "2026-10-16T12:00:00Z",
            })))
            .mount(&self.server)
            .await;
    }

    fn pull(&self) -> std::result::Result<SyncResult, CasError> {
        let tasks = open_task_store_local(&self.root).unwrap();
        let rules = open_rule_store_local(&self.root).unwrap();
        let skills = open_skill_store_local(&self.root).unwrap();
        if self.team {
            self.syncer.pull_team(
                TEAM,
                PROJECT,
                self.entries.as_ref(),
                tasks.as_ref(),
                rules.as_ref(),
                skills.as_ref(),
            )
        } else {
            self.syncer.pull(
                self.entries.as_ref(),
                tasks.as_ref(),
                rules.as_ref(),
                skills.as_ref(),
                open_spec_store(&self.root).unwrap().as_ref(),
                open_event_store(&self.root).unwrap().as_ref(),
                open_prompt_store(&self.root).unwrap().as_ref(),
                open_file_change_store(&self.root).unwrap().as_ref(),
                open_commit_link_store(&self.root).unwrap().as_ref(),
            )
        }
    }
}

fn legacy_pair() -> (Entry, Entry) {
    // These are persisted IDs minted by two independent legacy counters.
    let mut a = Entry::new("2026-10-15-6".into(), "checkout A's distinct memory".into());
    a.created = Utc.with_ymd_and_hms(2026, 10, 15, 10, 0, 0).unwrap();
    a.title = Some("First checkout".into());
    a.origin_project = Some(PROJECT.into());
    let mut b = Entry::new(a.id.clone(), "checkout B's distinct memory".into());
    b.created = Utc.with_ymd_and_hms(2026, 10, 15, 11, 0, 0).unwrap();
    b.title = Some("Second checkout".into());
    b.origin_project = Some(PROJECT.into());
    (a, b)
}

#[tokio::test]
async fn cas_25eb1_pull_preserves_distinct_legacy_memories_and_replays_once() {
    let _env = TestEnvGuard::temp_home();
    for team in [false, true] {
        let (a, b) = legacy_pair();
        let f = Fixture::new(team, &a, &b).await;
        let result = f.pull().unwrap();
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        let rows = f.entries.list().unwrap();
        assert_eq!(
            rows.len(),
            2,
            "same counter ID is not one memory; team={team}"
        );
        assert_eq!(f.entries.get(&a.id).unwrap().content, a.content);
        let recovered = rows.iter().find(|r| r.content == b.content).unwrap();
        let recovered_id = recovered.id.clone();
        assert!(!recovered.archived);
        assert_ne!(recovered_id, b.id);
        let result = f.pull().unwrap();
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!(
            f.entries.list().unwrap().len(),
            2,
            "replay must not mint a third ID"
        );
        assert_eq!(f.entries.get(&recovered_id).unwrap().title, b.title);
        let queued = if team {
            f.syncer.queue.pending_for_team(TEAM, 20, 5).unwrap()
        } else {
            f.syncer.queue.pending(20, 5).unwrap()
        };
        assert_eq!(
            queued.len(),
            1,
            "the recovered identity must sync exactly once"
        );
        assert_eq!(queued[0].entity_id, recovered_id);
        // The remote's revision belongs to its old storage key, not our
        // incumbent or the newly minted recovery key.
        assert_eq!(
            f.syncer.queue.revision(EntityType::Entry, &a.id).unwrap(),
            Some(3)
        );
        assert_eq!(
            f.syncer
                .queue
                .revision(EntityType::Entry, &recovered_id)
                .unwrap(),
            None
        );
    }
}

#[tokio::test]
async fn cas_25eb1_matching_birth_edits_and_identical_memories_still_dedupe() {
    let _env = TestEnvGuard::temp_home();
    for team in [false, true] {
        for same_birth in [false, true] {
            let (a, mut b) = legacy_pair();
            if same_birth {
                b.created = a.created;
            } else {
                b.content = a.content.clone();
                b.title = a.title.clone();
            }
            let f = Fixture::new(team, &a, &b).await;
            let result = f.pull().unwrap();
            assert!(result.errors.is_empty(), "{:?}", result.errors);
            assert_eq!(f.entries.list().unwrap().len(), 1);
            assert_eq!(f.entries.get(&a.id).unwrap().content, b.content);
            assert!(f.syncer.queue.list_all(10).unwrap().is_empty());
        }
    }
}
