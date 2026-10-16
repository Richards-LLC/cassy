//! Legacy memory collisions through both real HTTP pull routes (cas-25eb1).
use super::*;
use crate::store::*;
use crate::test_support::TestEnvGuard;
use crate::types::Entry;
use chrono::{TimeZone, Utc};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PROJECT: &str = "gabber-studio";
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
        let mut wire = serde_json::to_value(remote).unwrap();
        wire["project_id"] = serde_json::json!(PROJECT);
        wire["updated_at"] = serde_json::json!("2099-10-16T12:00:00Z");
        wire["revision"] = serde_json::json!("7");
        self.respond_wires(vec![wire], "2026-10-16T12:00:00Z").await;
    }

    async fn respond_wires(&self, wires: Vec<serde_json::Value>, pulled_at: &str) {
        self.server.reset().await;
        let endpoint = if self.team {
            format!("/api/teams/{TEAM}/sync/pull")
        } else {
            super::pull::PULL_PATH.into()
        };
        Mock::given(method("GET"))
            .and(path(endpoint))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "entries": wires, "tasks": [], "rules": [], "skills": [],
                "pulled_at": pulled_at,
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

// pin: literal IDs from Cloud 193102b memoryIdentitySha256/rekeyedMemoryId,
// including #163's Beta/Alpha vectors. Cassy never computes alias-fold IDs.
fn cloud_vectors(team: bool) -> Vec<serde_json::Value> {
    let vectors: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("cloud_rekey_vectors.json")).unwrap();
    vectors
        .into_iter()
        .filter(|v| v["scope"]["kind"] == if team { "team" } else { "personal" })
        .map(|v| {
            let mut wire = v["data"].clone();
            wire["project_id"] = serde_json::json!(PROJECT);
            wire["origin_project"] = serde_json::json!(PROJECT);
            wire["updated_at"] = serde_json::json!("2099-10-16T12:00:00Z");
            wire["revision"] = serde_json::json!("7");
            wire
        })
        .collect()
}

#[tokio::test]
async fn cas_e29c_cloud_fold_vectors_round_trip_store_search_edit_push_replay() {
    use crate::hybrid_search::{SearchIndex, SearchOptions};
    use flate2::read::GzDecoder;
    use std::io::Read;
    let _env = TestEnvGuard::temp_home();
    for team in [false, true] {
        let (mut keeper, remote) = legacy_pair();
        keeper.id = "2026-10-01-1".into();
        let f = Fixture::new(team, &keeper, &remote).await;
        let wires = cloud_vectors(team);
        f.respond_wires(wires.clone(), "2026-10-16T12:00:00Z").await;
        let result = f.pull().unwrap();
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!(result.pulled_entries, wires.len());
        let index = SearchIndex::in_memory().unwrap();
        index
            .index_entries_batch(&f.entries.list().unwrap())
            .unwrap();
        let editing = SyncingEntryStore::new(f.entries.clone(), f.syncer.queue.clone())
            .with_cloud_config(Arc::new(CloudConfig {
                team_id: team.then(|| TEAM.into()),
                team_only: team,
                ..Default::default()
            }));
        let mut echoed = Vec::new();
        for wire in &wires {
            let id = wire["id"].as_str().unwrap();
            let mut entry = editing.get(id).unwrap();
            let hits = index
                .search(
                    &SearchOptions {
                        query: wire["content"].as_str().unwrap().into(),
                        ..Default::default()
                    },
                    &f.entries.list().unwrap(),
                )
                .unwrap();
            assert!(hits.iter().any(|hit| hit.id == id), "not searchable: {id}");
            entry.content.push_str(" edited in Cassy");
            editing.update(&entry).unwrap();
            let mut echo = serde_json::to_value(editing.get(id).unwrap()).unwrap();
            assert_eq!(
                echo["id"], wire["id"],
                "opaque ID must remain byte-identical"
            );
            assert_eq!(echo.get("created").is_some(), wire.get("created").is_some());
            echo["project_id"] = serde_json::json!(PROJECT);
            echo["updated_at"] = serde_json::json!("2099-10-17T12:00:00Z");
            echo["revision"] = serde_json::json!("8");
            echoed.push(echo);
        }
        let push_path = if team {
            format!("/api/teams/{TEAM}/sync/push")
        } else {
            "/api/sync/push".into()
        };
        Mock::given(method("POST"))
            .and(path(&push_path))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "entries": {"inserted": 0, "updated": wires.len(), "skipped": 0},
                "synced": {"entries": {"inserted": 0, "updated": wires.len(), "skipped": 0}}
            })))
            .expect(1)
            .mount(&f.server)
            .await;
        let pushed = if team {
            f.syncer.push_team(TEAM)
        } else {
            f.syncer.push()
        }
        .unwrap();
        assert!(pushed.errors.is_empty(), "{:?}", pushed.errors);
        assert_eq!(pushed.pushed_entries, wires.len());
        let requests = f.server.received_requests().await.unwrap();
        let request = requests.iter().find(|r| r.url.path() == push_path).unwrap();
        let mut json = String::new();
        GzDecoder::new(request.body.as_slice())
            .read_to_string(&mut json)
            .unwrap();
        let payload: serde_json::Value = serde_json::from_str(&json).unwrap();
        let sent = payload["entries"].as_array().unwrap();
        for wire in &wires {
            let row = sent.iter().find(|row| row["id"] == wire["id"]).unwrap();
            assert_eq!(row.get("created").is_some(), wire.get("created").is_some());
            assert!(
                row["content"]
                    .as_str()
                    .unwrap()
                    .ends_with("edited in Cassy")
            );
        }
        f.respond_wires(echoed, "2026-10-17T12:00:00Z").await;
        for _ in 0..2 {
            assert!(f.pull().unwrap().errors.is_empty());
        }
        assert_eq!(f.entries.list().unwrap().len(), wires.len() + 1);
        assert!(f.entries.list_archived().unwrap().is_empty());
        assert!(f.syncer.queue.list_all(20).unwrap().is_empty());
        assert_eq!(f.entries.get(&keeper.id).unwrap().content, keeper.content);
    }
}

#[tokio::test]
async fn cas_e29c_occupied_fold_preserves_both_bodies_and_never_stalls_pull() {
    let _env = TestEnvGuard::temp_home();
    for team in [false, true] {
        for missing_created in [false, true] {
            let mut wire = cloud_vectors(team)[if missing_created { 2 } else { 0 }].clone();
            let (mut incumbent, remote) = legacy_pair();
            incumbent.id = wire["id"].as_str().unwrap().into();
            let f = Fixture::new(team, &incumbent, &remote).await;
            if missing_created {
                let mut local = serde_json::to_value(&incumbent).unwrap();
                local.as_object_mut().unwrap().remove("created");
                f.entries
                    .update(&serde_json::from_value(local).unwrap())
                    .unwrap();
            }
            f.respond_wires(vec![wire.clone()], "2026-10-16T12:00:00Z")
                .await;
            let result = f.pull().unwrap();
            assert!(
                result.errors.is_empty(),
                "preserved conflict is not a pull error"
            );
            assert_eq!(result.conflicts_resolved_local, 1);
            assert_eq!(
                serde_json::to_value(&result).unwrap()["memory_fold_conflicts"][0],
                wire["id"]
            );
            assert_eq!(
                f.entries.get(&incumbent.id).unwrap().content,
                incumbent.content
            );
            let records = f.syncer.queue.list_conflicts(20).unwrap();
            let record = records
                .iter()
                .find(|r| r.strategy == "fold_conflict")
                .unwrap();
            let before: serde_json::Value =
                serde_json::from_str(&record.discarded_row_json).unwrap();
            assert_eq!(before["incoming"]["content"], wire["content"]);
            assert_eq!(before["local"]["content"], incumbent.content);
            assert!(f.entries.list_archived().unwrap().is_empty());
            let watermark = if team {
                format!("last_team_pull_at_{TEAM}_{PROJECT}")
            } else {
                "last_pull_at".into()
            };
            assert_eq!(
                f.syncer.queue.get_metadata(&watermark).unwrap().as_deref(),
                Some("2026-10-16T12:00:00Z")
            );
            wire["id"] = serde_json::json!("subsequent-opaque-memory");
            wire["content"] = serde_json::json!("later pull still syncs");
            f.respond_wires(vec![wire], "2026-10-17T12:00:00Z").await;
            assert_eq!(f.pull().unwrap().pulled_entries, 1);
            assert_eq!(
                f.entries.get("subsequent-opaque-memory").unwrap().content,
                "later pull still syncs"
            );
            assert_eq!(
                f.syncer.queue.get_metadata(&watermark).unwrap().as_deref(),
                Some("2026-10-17T12:00:00Z")
            );
        }
    }
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
        f.syncer.queue.mark_synced(queued[0].id).unwrap();
        f.pull().unwrap();
        assert!(
            f.syncer.queue.list_all(20).unwrap().is_empty(),
            "an acknowledged recovery must not enqueue again on replay"
        );
        let mut edited = b.clone();
        edited.content = "checkout B's updated memory".into();
        f.respond(&edited).await;
        f.pull().unwrap();
        assert_eq!(
            f.entries.get(&recovered_id).unwrap().content,
            edited.content
        );
        assert_eq!(
            f.entries.list().unwrap().len(),
            2,
            "mutable content must not change the recovery ID"
        );
    }
}

#[tokio::test]
async fn cas_25eb1_archived_incumbent_and_occupied_recovery_are_preserved() {
    let _env = TestEnvGuard::temp_home();
    for team in [false, true] {
        let (a, b) = legacy_pair();
        let f = Fixture::new(team, &a, &b).await;
        f.entries.archive(&a.id).unwrap();
        f.pull().unwrap();
        assert_eq!(f.entries.get_archived(&a.id).unwrap().content, a.content);
        assert_eq!(f.entries.list().unwrap().len(), 1);

        let f = Fixture::new(team, &a, &b).await;
        let mut blocker = Entry::new(
            super::entry_identity::recovery_id(&b),
            "occupied recovery".into(),
        );
        blocker.created = a.created;
        f.entries.add(&blocker).unwrap();
        let error = f.pull().unwrap_err().to_string();
        assert!(error.contains("belongs to another creation"), "{error}");
        assert_eq!(f.entries.get(&a.id).unwrap().content, a.content);
        assert_eq!(f.entries.get(&blocker.id).unwrap().content, blocker.content);
        assert!(f.syncer.queue.list_all(20).unwrap().is_empty());
        assert!(
            f.syncer
                .queue
                .get_metadata("last_pull_at")
                .unwrap()
                .is_none()
        );
    }
}

#[tokio::test]
async fn cas_25eb1_unattributed_collision_is_preserved_without_republishing() {
    let _env = TestEnvGuard::temp_home();
    for team in [false, true] {
        let (a, mut b) = legacy_pair();
        b.origin_project = None;
        let f = Fixture::new(team, &a, &b).await;
        f.pull().unwrap();
        assert_eq!(f.entries.list().unwrap().len(), 2);
        assert!(f.syncer.queue.list_all(20).unwrap().is_empty());
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

#[tokio::test]
async fn cas_e29c_fold_same_memory_keeps_lww_and_custom_suffixes_stay_opaque() {
    let _env = TestEnvGuard::temp_home();
    for team in [false, true] {
        for id in [
            "custom-source-fold0123456789ab",
            "custom-foldnot-a-v1-digest",
        ] {
            for same_birth in [false, true] {
                let (mut a, mut b) = legacy_pair();
                a.id = id.into();
                b.id = id.into();
                if same_birth {
                    b.created = a.created;
                } else {
                    b.title = a.title.clone();
                    b.content = a.content.clone();
                }
                let f = Fixture::new(team, &a, &b).await;
                assert!(f.pull().unwrap().errors.is_empty());
                assert_eq!(f.entries.list().unwrap().len(), 1);
                assert_eq!(f.entries.get(id).unwrap().created, b.created);
                assert_eq!(f.entries.get(id).unwrap().content, b.content);
                assert!(
                    f.syncer
                        .queue
                        .memory_fold_conflict_ids()
                        .unwrap()
                        .is_empty()
                );
            }
        }
    }
}

#[tokio::test]
async fn cas_e29c_archived_fold_incumbent_stays_archived_with_incoming_journaled() {
    let _env = TestEnvGuard::temp_home();
    for team in [false, true] {
        let wire = cloud_vectors(team)[0].clone();
        let (mut local, remote) = legacy_pair();
        local.id = wire["id"].as_str().unwrap().into();
        let f = Fixture::new(team, &local, &remote).await;
        f.entries.archive(&local.id).unwrap();
        f.respond_wires(vec![wire.clone()], "2026-10-16T12:00:00Z")
            .await;
        let result = f.pull().unwrap();
        assert!(result.errors.is_empty());
        assert_eq!(result.memory_fold_conflicts, vec![local.id.clone()]);
        assert_eq!(
            f.entries.get_archived(&local.id).unwrap().content,
            local.content
        );
        assert!(f.entries.list().unwrap().is_empty());
        let conflicts = f.syncer.queue.list_conflicts(20).unwrap();
        let before: serde_json::Value =
            serde_json::from_str(&conflicts[0].discarded_row_json).unwrap();
        assert_eq!(before["incoming"]["content"], wire["content"]);
        assert_eq!(before["local"]["archived"], true);
    }
}

#[tokio::test]
async fn cas_e29c_missing_birth_legacy_collisions_use_distinct_stable_local_recovery() {
    let _env = TestEnvGuard::temp_home();
    for team in [false, true] {
        let (a, b) = legacy_pair();
        let mut local = serde_json::to_value(&a).unwrap();
        local.as_object_mut().unwrap().remove("created");
        let local: Entry = serde_json::from_value(local).unwrap();
        let f = Fixture::new(team, &local, &b).await;
        let mut wire = serde_json::to_value(&b).unwrap();
        wire.as_object_mut().unwrap().remove("created");
        wire["project_id"] = serde_json::json!(PROJECT);
        wire["updated_at"] = serde_json::json!("2099-10-16T12:00:00Z");
        f.respond_wires(vec![wire], "2026-10-16T12:00:00Z").await;
        for _ in 0..2 {
            assert!(f.pull().unwrap().errors.is_empty());
            let rows = f.entries.list().unwrap();
            assert_eq!(rows.len(), 2);
            assert_eq!(f.entries.get(&a.id).unwrap().content, a.content);
            let recovered = rows.iter().find(|e| e.content == b.content).unwrap();
            assert!(recovered.id.starts_with(&format!("{}-recovered-", a.id)));
            assert!(!recovered.has_known_creation());
        }
        assert!(f.entries.list_archived().unwrap().is_empty());
    }
}
