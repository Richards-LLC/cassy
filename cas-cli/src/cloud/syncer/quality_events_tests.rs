//! The client against a mock Cloud implementing the agreed contract exactly.
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex};

use cas_store::{InterventionKind, NewQualityEvent, QualityPayload};
use wiremock::{Match, Mock, MockServer, Request, Respond, ResponseTemplate};

use crate::cloud::{CloudConfig, CloudSyncer, CloudSyncerConfig, SyncQueue};

const TEAM: &str = "team-ledger";

#[derive(Default)]
struct CloudState {
    /// Ingest order; idempotent on id, first write wins.
    rows: Vec<serde_json::Value>,
    posts: Vec<usize>,
    gets: usize,
    /// Status to answer instead of serving, consumed front first.
    forced: Vec<u16>,
    /// Any POST over this many events answers 413.
    max_batch: Option<usize>,
}

#[derive(Clone, Default)]
struct MockCloud(Arc<Mutex<CloudState>>);

impl Respond for MockCloud {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let mut state = self.0.lock().unwrap();
        if !state.forced.is_empty() {
            return ResponseTemplate::new(state.forced.remove(0));
        }
        if request.method.as_str() == "GET" {
            state.gets += 1;
            let query: HashMap<_, _> = request.url.query_pairs().into_owned().collect();
            let start: usize = query.get("cursor").and_then(|c| c.parse().ok()).unwrap_or(0);
            let limit: usize = query.get("limit").and_then(|l| l.parse().ok()).unwrap_or(500);
            let page: Vec<_> = state.rows.iter().skip(start).take(limit).cloned().collect();
            let next = start + page.len();
            return ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "events": page,
                "next_cursor": next.to_string(),
            }));
        }
        let mut body = String::new();
        flate2::read::GzDecoder::new(request.body.as_slice())
            .read_to_string(&mut body)
            .unwrap();
        let envelope: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(
            envelope["project_canonical_id"].is_string(),
            "a bad envelope is a 400 for the whole batch"
        );
        let events = envelope["events"].as_array().unwrap();
        assert!(events.len() <= 200, "over 200 events is a 400");
        if state.max_batch.is_some_and(|max| events.len() > max) {
            return ResponseTemplate::new(413);
        }
        state.posts.push(events.len());
        let (mut accepted, mut duplicates) = (0, 0);
        for event in events {
            if state.rows.iter().any(|row| row["id"] == event["id"]) {
                duplicates += 1;
            } else {
                accepted += 1;
                let mut stored = event.clone();
                stored["received_at"] = serde_json::json!("2026-10-31T19:00:00Z");
                stored["reporter_user_id"] = serde_json::json!("user-from-token");
                state.rows.push(stored);
            }
        }
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "accepted": accepted, "duplicates": duplicates, "rejected": [],
        }))
    }
}

struct TeamRoute;

impl Match for TeamRoute {
    fn matches(&self, request: &Request) -> bool {
        request.url.path() == format!("/api/teams/{TEAM}/quality-events")
    }
}

fn machine(root: &Path, endpoint: &str, project: &str) -> CloudSyncer {
    crate::cloud::set_canonical_id_in_config_toml(root, project).unwrap();
    let queue = Arc::new(SyncQueue::open(root).unwrap());
    queue.init().unwrap();
    let config = CloudConfig {
        endpoint: endpoint.to_string(),
        token: Some("test-token".to_string()),
        team_id: Some(TEAM.to_string()),
        ..Default::default()
    };
    CloudSyncer::new_for_project(
        queue,
        config,
        CloudSyncerConfig::default(),
        project.to_string(),
        root,
    )
}

fn emit(root: &Path, key: &str) {
    let mut event = NewQualityEvent::new(
        key,
        QualityPayload::Intervention {
            intervention: InterventionKind::OverrideClose,
            minutes: None,
        },
    );
    event.scope = "scope".to_string();
    event.task_id = Some("cas-1".to_string());
    event.title = Some("Fix `rm -rf /secret-path` flow".to_string());
    event.local_note = Some("the secret reason".to_string());
    assert!(cas_store::record_quality_event(root, &event).unwrap());
}

/// Machine A's event reaches machine B in one batch, carrying scope A only;
/// a teammate's project arrives attributed; re-syncing moves nothing.
#[tokio::test(flavor = "multi_thread")]
async fn events_reach_a_second_machine_in_batches_under_scope_a() {
    let server = MockServer::start().await;
    let cloud = MockCloud::default();
    Mock::given(TeamRoute).respond_with(cloud.clone()).mount(&server).await;
    let (a_dir, b_dir, c_dir) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let (a_root, b_root, c_root) =
        (a_dir.path().to_path_buf(), b_dir.path().to_path_buf(), c_dir.path().to_path_buf());
    let endpoint = server.uri();

    let (a_summary, b_summary, resync, stored) = tokio::task::spawn_blocking({
        let cloud = cloud.clone();
        move || {
            let a = machine(&a_root, &endpoint, "acme/this");
            let b = machine(&b_root, &endpoint, "acme/this");
            let teammate = machine(&c_root, &endpoint, "acme/other");
            for key in ["one", "two", "three"] {
                emit(&a_root, key);
            }
            emit(&c_root, "from another project");

            let a_summary = a.sync_quality_events(true).unwrap();
            teammate.sync_quality_events(true).unwrap();
            let b_summary = b.sync_quality_events(true).unwrap();
            let resync = a.sync_quality_events(true).unwrap();

            let stored = cas_store::quality_events_between(&b_root, None, None).unwrap();
            assert_eq!(cloud.0.lock().unwrap().posts, [3, 1], "one request per batch");
            (a_summary, b_summary, resync, stored)
        }
    })
    .await
    .unwrap();

    assert_eq!(a_summary.pushed, 3);
    assert_eq!(b_summary.pulled, 4, "{stored:?}");
    assert_eq!(resync.pushed, 0, "the push cursor advanced");
    // A's own rows came back as its own project; the teammate's stay attributed.
    assert_eq!(resync.pulled, 1);
    assert_eq!(stored.iter().filter(|event| event.project_id == "acme/other").count(), 1);
    assert_eq!(stored.iter().filter(|event| event.project_id.is_empty()).count(), 3);
    assert!(stored.iter().all(|event| event.from_cloud && event.local_note.is_none()));

    let rows = serde_json::to_string(&cloud.0.lock().unwrap().rows).unwrap();
    assert!(!rows.contains("secret"), "scope A: {rows}");
    assert!(rows.contains("\"title\":\"Fix flow\""), "{rows}");
    assert!(rows.contains("\"project_id\":\"acme/this\""), "{rows}");
}

/// Until the route is live nothing is lost or noisy; 413 splits the batch.
#[tokio::test(flavor = "multi_thread")]
async fn route_not_live_holds_the_cursor_and_413_splits_the_batch() {
    let server = MockServer::start().await;
    let cloud = MockCloud::default();
    Mock::given(TeamRoute).respond_with(cloud.clone()).mount(&server).await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let endpoint = server.uri();

    let state = cloud.clone();
    tokio::task::spawn_blocking(move || {
        let syncer = machine(&root, &endpoint, "acme/this");
        for key in ["one", "two", "three"] {
            emit(&root, key);
        }
        state.0.lock().unwrap().forced = vec![404];
        assert!(syncer.sync_quality_events(true).unwrap().capability_absent);
        assert!(state.0.lock().unwrap().rows.is_empty());
        // The daemon's paced call does not ask again while the route is absent.
        state.0.lock().unwrap().forced = vec![500];
        assert!(syncer.sync_quality_events(false).unwrap().capability_absent);
        assert_eq!(state.0.lock().unwrap().forced, [500], "no request was made");

        // Live, with a size limit: the held events all arrive, split.
        {
            let mut cloud = state.0.lock().unwrap();
            cloud.forced.clear();
            cloud.max_batch = Some(2);
        }
        let summary = syncer.sync_quality_events(true).unwrap();
        assert!(!summary.capability_absent);
        assert_eq!(summary.pushed, 3);
        let cloud = state.0.lock().unwrap();
        assert_eq!(cloud.posts, [1, 1, 1], "halved from 3 events to 1, then held there");
        assert_eq!(cloud.rows.len(), 3);
    })
    .await
    .unwrap();
}
