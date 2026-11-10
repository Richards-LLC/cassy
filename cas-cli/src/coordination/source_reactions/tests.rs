use super::*;
use cas_types::{Agent, AgentRole, Task, TaskStatus};
use serde_json::json;

// Production uses a live clock; fixtures hold each turn at its named instant.
fn reconcile(
    root: &Path,
    registration: &str,
    session: &str,
    now: DateTime<Utc>,
    call: &mut dyn FnMut(&str, Value) -> Result<Value, VioletError>,
) -> anyhow::Result<()> {
    reconcile_with_clock(root, registration, session, &|| now, call)
}

const SOURCE: &str = "https://example.slack.com/archives/C123/p1710000000000001";

struct World {
    _temp: tempfile::TempDir,
    root: std::path::PathBuf,
    now: DateTime<Utc>,
}

impl World {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(temp.path()).unwrap();
        std::fs::write(
            root.join("config.toml"),
            "[project]\ncanonical_id = 'fixture'\n",
        )
        .unwrap();
        let mut supervisor = Agent::new("sup".into(), "supervisor".into());
        supervisor.role = AgentRole::Supervisor;
        supervisor.factory_session = Some("session".into());
        crate::store::open_agent_store(&root)
            .unwrap()
            .register(&supervisor)
            .unwrap();
        let now = Utc::now();
        let journal = super::super::external_work::SqliteRequestJournal::open(&root).unwrap();
        journal
            .prepare(
                "claim",
                "claim",
                &json!({"project_id":"fixture","external_ref":SOURCE}),
            )
            .unwrap();
        journal
            .record_result(
                "claim",
                &json!({"code":"acquired","receipt":{
                    "resource":{"id":"resource","state":"claimed"},
                    "visible_owner":{"registration_id":"sup"}, "generation":1,
                    "expires_at":(now + chrono::Duration::hours(1)).to_rfc3339()
                }}),
                None,
            )
            .unwrap();
        let mut task = Task::new("cas-source".into(), "source work".into());
        task.external_ref = Some(SOURCE.into());
        crate::store::open_task_store(&root)
            .unwrap()
            .add(&task)
            .unwrap();
        Self {
            _temp: temp,
            root,
            now,
        }
    }

    fn status(&self, status: TaskStatus) {
        let store = crate::store::open_task_store(&self.root).unwrap();
        let mut task = store.get("cas-source").unwrap();
        task.status = status;
        store.update(&task).unwrap();
    }

    fn git(&self, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(self.root.parent().unwrap())
            .args([
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@example.invalid",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn delivery(&self) -> String {
        self.git(&["init", "-q", "-b", "main"]);
        self.git(&["commit", "--allow-empty", "-qm", "base"]);
        self.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        self.git(&["commit", "--allow-empty", "-qm", "delivery"]);
        let sha = self.git(&["rev-parse", "HEAD"]);
        let store = crate::store::open_task_store(&self.root).unwrap();
        let mut task = store.get("cas-source").unwrap();
        task.deliverables.factory_branch_anchor = Some(sha.clone());
        task.deliverables.green_merge_commit = Some(sha.clone());
        task.deliverables.work_target = Some(cas_types::WorkTarget {
            repo_selector: "project:fixture".into(),
            target_branch: "epic/train".into(),
        });
        store.update(&task).unwrap();
        sha
    }

    fn owner(
        &self,
        request: &str,
        owner: Option<&str>,
        state: &str,
        generation: u64,
        expires: DateTime<Utc>,
    ) {
        let journal = super::super::external_work::SqliteRequestJournal::open(&self.root).unwrap();
        journal
            .prepare(
                request,
                "claim_status",
                &json!({"project_id":"fixture","external_ref":SOURCE}),
            )
            .unwrap();
        journal
            .record_result(
                request,
                &json!({"code":"status","receipt":{
                    "resource":{"id":"resource","state":state},
                    "visible_owner":owner.map(|id| json!({"registration_id":id})), "generation":generation,
                    "expires_at":expires.to_rfc3339()
                }}),
                None,
            )
            .unwrap();
    }
}

fn read(marks: &[&str]) -> Value {
    json!({"ok":true,"channel":{"id":"C123"},"complete":true,"messages":[{
        "message_id":"1710000000.000001","permalink":SOURCE,
        "reactions":marks.iter().map(|name| json!({"name":name,"count":1,"users":["BOT"]})).collect::<Vec<_>>()
    }]})
}

fn posted(args: &Value) -> Value {
    json!({"ok":true,"reaction":{"name":args["reaction"],"action":args["action"],"changed":true}})
}

#[test]
fn source_reaction_lifecycle_reconciles_source_posts() {
    let _env = crate::test_support::TestEnvGuard::temp_home();
    let world = World::new();
    let sha = world.delivery();
    let mut observed = vec!["eyes".to_string(), "pick".to_string()];
    // A closed task without staging evidence must retain pick. A reopened
    // task reverses status; cancellation clears only the bot's status marks.
    for (step, status, expected) in [
        (0, TaskStatus::Open, Some("eyes")),
        (1, TaskStatus::InProgress, Some("pick")),
        (2, TaskStatus::AwaitingMerge, Some("pick")),
        (3, TaskStatus::Closed, Some("pick")),
        (4, TaskStatus::Closed, Some("white_check_mark")),
        (5, TaskStatus::Closed, Some("pick")),
        (6, TaskStatus::Closed, Some("white_check_mark")),
        (7, TaskStatus::Open, Some("eyes")),
        (8, TaskStatus::Cancelled, None),
    ] {
        if step == 4 {
            world.git(&["update-ref", "refs/remotes/origin/staging", &sha]);
            // The reply claim expired, then released. Its accepted owner
            // still has status-only authority through staging.
            world.owner(
                "expired",
                Some("sup"),
                "claimed",
                1,
                world.now - chrono::Duration::minutes(1),
            );
            world.owner(
                "released",
                None,
                "released",
                2,
                world.now - chrono::Duration::minutes(1),
            );
        }
        if step == 4 {
            let authority =
                super::super::external_work::SqliteRequestJournal::open(&world.root).unwrap();
            assert!(
                authority
                    .task_source_receipt(SOURCE, "fixture")
                    .unwrap()
                    .is_none()
            );
        }
        if step == 5 {
            let mut related = Task::new("cas-related".into(), "related work".into());
            related.external_ref = Some(format!("{SOURCE}?thread_ts=1710000000.000001"));
            crate::store::open_task_store(&world.root)
                .unwrap()
                .add(&related)
                .unwrap();
        }
        if step == 6 {
            let store = crate::store::open_task_store(&world.root).unwrap();
            let mut related = store.get("cas-related").unwrap();
            related.status = TaskStatus::Cancelled;
            store.update(&related).unwrap();
        }
        world.status(status);
        let mut calls = Vec::new();
        reconcile(&world.root, "sup", "session", world.now + chrono::Duration::minutes(step * 20), &mut |tool, args| {
            calls.push((tool.to_string(), args.clone()));
            if tool == "violet_read" {
                return Ok(json!({"ok":true,"channel":{"id":"C123"},"complete":true,"messages":[{
                    "message_id":"1710000000.000001","permalink":SOURCE,
                    "reactions":observed.iter().map(|name| json!({"name":name,"count":1,"users":["BOT"]})).collect::<Vec<_>>()
                }]}));
            }
            assert_eq!(tool, "violet_post");
            assert_eq!(args["kind"], "reaction");
            assert_eq!(args["channel"], "C123");
            assert_eq!(args["message_id"], "1710000000.000001");
            let mark = args["reaction"].as_str().unwrap().to_string();
            if args["action"] == "remove" { observed.retain(|name| name != &mark); }
            else if !observed.contains(&mark) { observed.push(mark.clone()); }
            Ok(json!({"ok":true,"reaction":{"name":mark,"action":args["action"],"changed":true}}))
        }).unwrap();
        assert!(
            !calls.is_empty(),
            "step {step}: no lifecycle reconciliation"
        );
        assert_eq!(
            observed,
            expected.into_iter().map(str::to_string).collect::<Vec<_>>(),
            "step {step}"
        );
        if matches!(step, 4 | 6 | 8) {
            // Terminal confirmations survive refresh age and a new reconciler
            // invocation. A changed related task above must still re-enter.
            let quiet_at = world.now + chrono::Duration::minutes(step * 20 + 10);
            reconcile(&world.root, "sup", "session", quiet_at, &mut |_, _| {
                panic!("terminal-confirmed source was polled again")
            })
            .unwrap();
            assert!(!audit_at(&world.root, false, quiet_at).unwrap()[0].pending);
            audit_at(&world.root, true, quiet_at).unwrap();
            let mut reads = 0;
            reconcile(&world.root, "sup", "session", quiet_at, &mut |tool, _| {
                assert_eq!(tool, "violet_read");
                reads += 1;
                Ok(read(&expected.into_iter().collect::<Vec<_>>()))
            })
            .unwrap();
            assert_eq!(reads, 1);
        }
    }
    // A later observed owner takeover immediately stops the former owner,
    // including a retry that was authorized under its expired claim.
    world.status(TaskStatus::Closed);
    world.owner(
        "takeover",
        Some("peer"),
        "claimed",
        3,
        world.now + chrono::Duration::hours(1),
    );
    reconcile(
        &world.root,
        "sup",
        "session",
        world.now + chrono::Duration::minutes(180),
        &mut |_, _| panic!("former source owner called Violet after observed takeover"),
    )
    .unwrap();
    let status = audit_at(&world.root, false, world.now)
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(status.owner.as_deref(), Some("peer"));
    assert!(status.pending);
    // A delayed response from before takeover must not revive the former
    // owner's authority merely because its journal row arrived last.
    world.owner(
        "late-prior-owner",
        Some("sup"),
        "claimed",
        1,
        world.now + chrono::Duration::hours(1),
    );
    assert_eq!(
        audit_at(&world.root, false, world.now).unwrap()[0]
            .owner
            .as_deref(),
        Some("peer")
    );
    reconcile(
        &world.root,
        "sup",
        "session",
        world.now + chrono::Duration::minutes(200),
        &mut |_, _| panic!("stale generation revived prior owner"),
    )
    .unwrap();
}

#[test]
fn source_reaction_retry_preserves_partial_receipts_and_reloads_state() {
    let _env = crate::test_support::TestEnvGuard::temp_home();
    let world = World::new();
    world.status(TaskStatus::InProgress);
    let limited = json!({"ok":false,"error":{"code":"rate_limited","retryable":true,"retry_after_seconds":120}});
    let mut operations = Vec::new();
    let clock = std::cell::Cell::new(world.now);
    reconcile_with_clock(
        &world.root,
        "sup",
        "session",
        &|| clock.get(),
        &mut |tool, args| {
            if tool == "violet_read" {
                return Ok(read(&["eyes", "white_check_mark"]));
            }
            operations.push((args["action"].clone(), args["reaction"].clone()));
            if args["reaction"] == "eyes" {
                Ok(posted(&args))
            } else {
                // Retry-After starts when the failed response arrives,
                // even when earlier calls consumed part of this turn.
                clock.set(world.now + chrono::Duration::seconds(30));
                Ok(limited.clone())
            }
        },
    )
    .unwrap();
    assert_eq!(
        operations,
        vec![
            (json!("remove"), json!("eyes")),
            (json!("remove"), json!("white_check_mark"))
        ]
    );
    let status = audit_at(&world.root, false, world.now)
        .unwrap()
        .pop()
        .unwrap();
    assert!(status.pending);
    assert_eq!(status.error.as_ref().unwrap(), &limited);
    assert_eq!(
        status.next_attempt,
        Some(world.now + chrono::Duration::seconds(150))
    );
    let db = Connection::open(world.root.join("cas.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM source_reaction_receipts", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        2
    );
    // A new process and a reverse transition retain Retry-After, including
    // when an operator rearms permanent failures via the audit command.
    world.status(TaskStatus::Open);
    audit_at(&world.root, true, world.now).unwrap();
    reconcile(
        &world.root,
        "sup",
        "session",
        world.now + chrono::Duration::seconds(60),
        &mut |_, _| panic!("retried before Retry-After"),
    )
    .unwrap();
    operations.clear();
    reconcile(
        &world.root,
        "sup",
        "session",
        world.now + chrono::Duration::seconds(150),
        &mut |tool, args| {
            if tool == "violet_read" {
                return Ok(read(&["white_check_mark"]));
            }
            operations.push((args["action"].clone(), args["reaction"].clone()));
            Ok(posted(&args))
        },
    )
    .unwrap();
    assert_eq!(
        operations,
        vec![
            (json!("remove"), json!("white_check_mark")),
            (json!("add"), json!("eyes"))
        ]
    );
    assert!(!audit_at(&world.root, false, world.now).unwrap()[0].pending);
    // A failure on the next read leaves a durable permanent refusal. It is
    // visible to audit and requires explicit rearming, not an endless retry.
    reconcile(
        &world.root,
        "sup",
        "session",
        world.now + chrono::Duration::seconds(210),
        &mut |_, _| Ok(json!({"ok":false,"error":{"code":"denied_by_policy","retryable":false}})),
    )
    .unwrap();
    reconcile(
        &world.root,
        "sup",
        "session",
        world.now + chrono::Duration::seconds(300),
        &mut |_, _| panic!("retried permanent refusal"),
    )
    .unwrap();
    audit_at(&world.root, true, world.now).unwrap();
    reconcile(
        &world.root,
        "sup",
        "session",
        world.now + chrono::Duration::seconds(301),
        &mut |tool, _| {
            assert_eq!(tool, "violet_read");
            Ok(read(&["eyes"]))
        },
    )
    .unwrap();
    assert!(!audit_at(&world.root, false, world.now).unwrap()[0].pending);
    assert!(
        audit_at(
            &world.root,
            false,
            world.now + chrono::Duration::seconds(422)
        )
        .unwrap()[0]
            .pending
    );
    // Removing the bot's own reaction cannot erase a human's same-name
    // reaction. Audit must continue to expose that visible disagreement.
    world.status(TaskStatus::InProgress);
    reconcile(
        &world.root,
        "sup",
        "session",
        world.now + chrono::Duration::seconds(400),
        &mut |tool, args| {
            if tool == "violet_read" {
                let mut response = read(&["eyes"]);
                response["messages"][0]["reactions"][0]["count"] = json!(2);
                response["messages"][0]["reactions"][0]["users"] = json!(["BOT", "HUMAN"]);
                return Ok(response);
            }
            Ok(posted(&args))
        },
    )
    .unwrap();
    let status = audit_at(&world.root, false, world.now)
        .unwrap()
        .pop()
        .unwrap();
    assert!(status.pending);
    assert!(status.observed.iter().any(|mark| mark == "eyes"));
}

#[test]
fn source_reaction_authority_and_identity_fences() {
    let _env = crate::test_support::TestEnvGuard::temp_home();
    let world = World::new();
    let agents = crate::store::open_agent_store(&world.root).unwrap();
    let mut agent = agents.get("sup").unwrap();
    for (step, role, status, session, allowed) in [
        (
            0,
            AgentRole::Standard,
            cas_types::AgentStatus::Active,
            "session",
            false,
        ),
        (
            1,
            AgentRole::Director,
            cas_types::AgentStatus::Active,
            "session",
            false,
        ),
        (
            2,
            AgentRole::Worker,
            cas_types::AgentStatus::Active,
            "session",
            false,
        ),
        (
            3,
            AgentRole::Supervisor,
            cas_types::AgentStatus::Shutdown,
            "session",
            false,
        ),
        (
            4,
            AgentRole::Supervisor,
            cas_types::AgentStatus::Stale,
            "session",
            false,
        ),
        (
            5,
            AgentRole::Supervisor,
            cas_types::AgentStatus::Active,
            "other-session",
            false,
        ),
        (
            6,
            AgentRole::Supervisor,
            cas_types::AgentStatus::Idle,
            "session",
            true,
        ),
        (
            7,
            AgentRole::Supervisor,
            cas_types::AgentStatus::Active,
            "session",
            true,
        ),
    ] {
        agent.role = role;
        agent.status = status;
        agent.factory_session = Some(session.into());
        agents.update(&agent).unwrap();
        audit_at(&world.root, true, world.now).unwrap();
        let mut calls = 0;
        reconcile(
            &world.root,
            "sup",
            "session",
            world.now + chrono::Duration::minutes(step),
            &mut |tool, _| {
                calls += 1;
                assert_eq!(tool, "violet_read");
                reconcile(
                    &world.root,
                    "sup",
                    "session",
                    world.now + chrono::Duration::minutes(step),
                    &mut |_, _| panic!("concurrent reconciler bypassed the source lease"),
                )
                .unwrap();
                let mut response = read(&["eyes"]);
                response["messages"][0]["permalink"] =
                    json!("https://other.slack.com/archives/C123/p1710000000000001");
                Ok(response)
            },
        )
        .unwrap();
        assert_eq!(calls, usize::from(allowed));
    }
    assert_eq!(
        audit_at(&world.root, false, world.now).unwrap()[0]
            .error
            .as_ref()
            .unwrap()["error"]["code"],
        "identity_mismatch"
    );
    audit_at(&world.root, true, world.now).unwrap();
    // Observe takeover during the read itself: no stale reaction may follow.
    reconcile(
        &world.root,
        "sup",
        "session",
        world.now + chrono::Duration::minutes(9),
        &mut |tool, _| {
            assert_eq!(tool, "violet_read");
            world.owner(
                "mid-read-takeover",
                Some("peer"),
                "claimed",
                2,
                world.now + chrono::Duration::hours(1),
            );
            Ok(read(&[]))
        },
    )
    .unwrap();
    assert_eq!(
        audit_at(&world.root, false, world.now).unwrap()[0]
            .owner
            .as_deref(),
        Some("peer")
    );
}

#[test]
fn source_reaction_deadline_defers_remaining_calls() {
    let _env = crate::test_support::TestEnvGuard::temp_home();
    let world = World::new();
    world.status(TaskStatus::InProgress);
    let clock = std::cell::Cell::new(world.now);
    let mut calls = Vec::new();
    reconcile_with_clock(
        &world.root,
        "sup",
        "session",
        &|| clock.get(),
        &mut |tool, args| {
            calls.push(tool.to_string());
            if tool == "violet_read" {
                clock.set(world.now + chrono::Duration::seconds(6));
                Ok(read(&["eyes", "white_check_mark"]))
            } else {
                Ok(posted(&args))
            }
        },
    )
    .unwrap();
    assert_eq!(calls, vec!["violet_read"]);
    assert!(audit_at(&world.root, false, clock.get()).unwrap()[0].pending);
    // The deferred operations are still due on a later pass.
    reconcile(
        &world.root,
        "sup",
        "session",
        world.now + chrono::Duration::seconds(66),
        &mut |tool, args| {
            if tool == "violet_read" {
                Ok(read(&["eyes"]))
            } else {
                Ok(posted(&args))
            }
        },
    )
    .unwrap();
    assert!(!audit_at(&world.root, false, world.now).unwrap()[0].pending);

    #[cfg(unix)]
    {
        use crate::bounded_process::Deadline;
        use cas_store::KnownRepoStore;
        use std::time::{Duration, Instant};
        let slow = World::new();
        let sha = slow.delivery();
        slow.status(TaskStatus::Closed);
        slow.git(&["update-ref", "refs/remotes/origin/staging", &sha]);
        let repo = slow.root.parent().unwrap().canonicalize().unwrap();
        let registry = crate::store::known_repos::open_host_known_repo_store().unwrap();
        registry.init().unwrap();
        registry
            .bind("project:fixture", &repo, &repo.join(".git"))
            .unwrap();
        let bin = slow.root.join("slow-bin");
        std::fs::create_dir(&bin).unwrap();
        let marker = bin.join("resolution-called");
        crate::test_paths::warm_stub(
            &bin.join("git"),
            &format!(
                "#!/bin/sh\nprintf x >> '{}'\nsleep 2\nexit 1\n",
                marker.display()
            ),
        );
        let task = crate::store::open_task_store(&slow.root)
            .unwrap()
            .get("cas-source")
            .unwrap();
        let started = Instant::now();
        let deadline = Deadline::after(Duration::from_millis(250));
        let probe =
            BoundedRepoProbe::new(deadline, PASS_BUDGET, 256).with_git_program(bin.join("git"));
        assert!(!staged(&slow.root, &task, deadline, &probe));
        assert!(marker.exists());
        assert!(deadline.remaining().is_zero());
        assert!(started.elapsed() < Duration::from_millis(1500));
        // A later pass with a fresh deadline can prove staging and reconcile.
        let mut reads = 0;
        reconcile(
            &slow.root,
            "sup",
            "session",
            slow.now + chrono::Duration::seconds(60),
            &mut |tool, args| {
                if tool == "violet_read" {
                    reads += 1;
                    Ok(read(&["white_check_mark"]))
                } else {
                    Ok(posted(&args))
                }
            },
        )
        .unwrap();
        assert_eq!(reads, 1);
    }
}
