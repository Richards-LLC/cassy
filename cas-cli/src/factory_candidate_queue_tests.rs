//! Through the real path: an MCP `factory action=candidate_request` from a
//! registered worker holding a lease, then the daemon's runner. Only the
//! compile is substituted, by a stub at the `cas qa candidate build` boundary.

use std::path::Path;
use std::process::Command;

use cas_mcp::types::CoordinationRequest;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::RawContent;
use tempfile::TempDir;

use super::*;
use crate::mcp::{CasCore, CasService};
use crate::test_support::TestEnvGuard;
use crate::types::{Agent, AgentRole, Task, TaskStatus};

const OWN_TASK: &str = "cas-own1";
const OTHER_TASK: &str = "cas-oth1";
const WORKER: &str = "robin";

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git").arg("-C").arg(repo).args(args).output().unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

struct Fixture {
    _env: TestEnvGuard,
    temp: TempDir,
    project: std::path::PathBuf,
    checkout: std::path::PathBuf,
    cas_root: std::path::PathBuf,
    service: CasService,
    /// The commit at the worker's HEAD.
    head: String,
    /// The commit before it, also reachable.
    parent: String,
    /// origin's default branch, which the project's main checkout lacks.
    source_tip: String,
}

impl Fixture {
    fn new() -> Self {
        let env = TestEnvGuard::temp_home();
        let temp = TempDir::new().unwrap();
        let origin = temp.path().join("origin.git");
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        git(temp.path(), &["init", "-q", "--bare", "-b", "main", origin.to_str().unwrap()]);
        git(&project, &["init", "-q", "-b", "main"]);
        git(&project, &["config", "user.name", "Fixture"]);
        git(&project, &["config", "user.email", "fixture@example.invalid"]);
        git(&project, &["remote", "add", "origin", origin.to_str().unwrap()]);
        std::fs::create_dir(project.join("scripts")).unwrap();
        std::fs::write(project.join("scripts/qa-candidate-build.py"), "# builder\n").unwrap();
        git(&project, &["add", "."]);
        git(&project, &["commit", "-qm", "builder"]);
        let stale = git(&project, &["rev-parse", "HEAD"]);
        git(&project, &["commit", "-q", "--allow-empty", "-m", "fixed builder"]);
        let source_tip = git(&project, &["rev-parse", "HEAD"]);
        git(&project, &["push", "-q", "origin", "main"]);
        git(&project, &["reset", "-q", "--hard", &stale]);

        let cas_root = crate::store::init_cas_dir(&project).unwrap();
        let checkout = cas_root.join("worktrees").join(WORKER);
        git(&project, &["worktree", "add", "-q", "-b", "factory/robin", checkout.to_str().unwrap()]);
        git(&checkout, &["commit", "-q", "--allow-empty", "-m", "first proof"]);
        let parent = git(&checkout, &["rev-parse", "HEAD"]);
        git(&checkout, &["commit", "-q", "--allow-empty", "-m", "second proof"]);
        let head = git(&checkout, &["rev-parse", "HEAD"]);

        let agents = crate::store::open_agent_store(&cas_root).unwrap();
        let worker = Agent::new_with_role("robin-id".into(), WORKER.into(), AgentRole::Worker);
        agents.register(&worker).unwrap();
        agents.register(&Agent::new_with_role("other-id".into(), "other".into(), AgentRole::Worker)).unwrap();
        let tasks = crate::store::open_task_store(&cas_root).unwrap();
        for (id, assignee, holder) in [(OWN_TASK, WORKER, "robin-id"), (OTHER_TASK, "other", "other-id")] {
            let mut task = Task::new(id.into(), "fixture".into());
            task.status = TaskStatus::InProgress;
            task.assignee = Some(assignee.into());
            tasks.add(&task).unwrap();
            agents.try_claim(id, holder, 600, None).unwrap();
        }
        let core = CasCore::with_daemon(cas_root.clone(), None, None);
        core.set_agent_id_for_testing("robin-id".to_string());
        mark_runner_live(&cas_root).unwrap();
        Self {
            _env: env,
            project,
            checkout,
            service: CasService::new(core, None),
            cas_root,
            head,
            parent,
            source_tip,
            temp,
        }
    }

    async fn request(&self, task: &str, revision: &str) -> String {
        let request: CoordinationRequest = serde_json::from_value(serde_json::json!({
            "action": "candidate_request", "task_id": task, "revision": revision,
        }))
        .unwrap();
        match self.service.factory(Parameters(request)).await {
            Ok(result) => result
                .content
                .iter()
                .filter_map(|content| match &content.raw {
                    RawContent::Text(text) => Some(text.text.clone()),
                    _ => None,
                })
                .collect(),
            Err(error) => format!("MCP_ERROR: {error}"),
        }
    }

    /// A stand-in for `cas qa candidate build`: records how it was invoked,
    /// then runs `body`, where `$7` is the revision.
    fn runner(&self, body: &str) -> Runner {
        let program = self.temp.path().join("cas-stub");
        let calls = self.temp.path().join("calls");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\necho \"$* | $(pwd) | $(git rev-parse HEAD) | $CAS_AGENT_ROLE | $CAS_ROOT\" >> '{}'\n{body}\n",
                calls.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(
            &program,
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        Runner {
            cas_root: self.cas_root.clone(),
            program,
            shutdown: Arc::new(AtomicBool::new(false)),
            admit: |root| open_lock(&root.join("test-slot")).map_err(|error| error.to_string()),
            build_elapsed: |started| started.elapsed(),
            heartbeat_interval: HEARTBEAT_INTERVAL,
            git_program: PathBuf::from("git"),
        }
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.temp.path().join("calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn messages(&self) -> Vec<(String, String)> {
        crate::store::open_prompt_queue_store(&self.cas_root)
            .unwrap()
            .peek_all(100)
            .unwrap()
            .into_iter()
            .map(|prompt| (prompt.target, prompt.prompt))
            .collect()
    }
}

const BUILT: &str = r#"echo "{\"candidate_dir\":\"/art/$7\",\"receipt_file\":\"/art/$7/RECEIPT.json\",\"revision\":\"$7\",\"version\":\"cas 9.9.9\",\"sha256\":\"hash-of-$7\"}""#;

#[tokio::test]
async fn worker_request_builds_in_the_daemon_and_tells_only_the_worker() {
    let mut fixture = Fixture::new();
    fixture._env.set("CAS_AGENT_ROLE", "supervisor");
    fixture._env.set("NEON_API_KEY", "fixture-secret");
    let store = fixture.temp.path().join("operator-store");
    std::fs::create_dir(&store).unwrap();
    std::fs::write(store.join("auth.json"), "fixture credential").unwrap();
    fixture._env.set("VERCEL_CONFIG_DIR", &store);
    let reply = fixture.request(OWN_TASK, &fixture.head).await;
    assert!(reply.contains("Queued"), "{reply}");
    assert!(fixture.calls().is_empty(), "the request path never builds");

    let runner = fixture.runner(&format!(
        "test -z \"$NEON_API_KEY\" || exit 41\ntest ! -s '{}/auth.json' || exit 42\n{BUILT}",
        store.display(),
    ));
    assert!(runner.run_next().unwrap());

    let calls = fixture.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    let builder = fixture.cas_root.join(BUILDER_CHECKOUT);
    let expected = format!(
        "qa candidate build --task {OWN_TASK} --rev {} --json | {} | {} |  | {}",
        fixture.head,
        builder.canonicalize().unwrap().display(),
        fixture.source_tip,
        fixture.cas_root.display(),
    );
    assert_eq!(calls[0], expected, "fixed builder, no supervisor authority");

    let messages = fixture.messages();
    assert_eq!(messages.len(), 1, "no supervisor turn: {messages:?}");
    assert_eq!(messages[0].0, WORKER);
    for part in [
        format!("/art/{}/cas", fixture.head),
        "cas 9.9.9".to_owned(),
        fixture.head.clone(),
        format!("hash-of-{}", fixture.head),
        format!("/art/{}/RECEIPT.json", fixture.head),
        format!("candidate-build-{}.log", &fixture.head[..12]),
    ] {
        assert!(messages[0].1.contains(&part), "{part} in {}", messages[0].1);
    }
    assert!(!runner.run_next().unwrap(), "the queue is drained");
    assert_eq!(fixture.calls().len(), 1);
}

#[tokio::test]
async fn refused_requests_name_why_and_queue_nothing() {
    let fixture = Fixture::new();
    // Reachable from the project's own history, never from this worker's HEAD.
    git(&fixture.project, &["commit", "-q", "--allow-empty", "-m", "not the worker's"]);
    let foreign = git(&fixture.project, &["rev-parse", "HEAD"]);
    let cases = [
        (OTHER_TASK, fixture.head.as_str(), "is not robin's active task"),
        (OWN_TASK, "HEAD", "full 40-character commit SHA"),
        (OWN_TASK, foreign.as_str(), "not reachable from HEAD of your checkout"),
    ];
    for (task, revision, why) in cases {
        let reply = fixture.request(task, revision).await;
        assert!(reply.contains("refused") && reply.contains(why), "{why}: {reply}");
    }
    std::fs::remove_file(queue_dir(&fixture.cas_root).join(RUNNER_HEARTBEAT)).unwrap();
    let reply = fixture.request(OWN_TASK, &fixture.head).await;
    assert!(reply.contains("no factory daemon"), "{reply}");

    let queued = fixture.runner(BUILT).queued();
    assert!(queued.is_empty(), "{queued:?}");
    assert!(fixture.checkout.is_dir());
}

#[tokio::test]
async fn newer_request_supersedes_the_queued_one() {
    let fixture = Fixture::new();
    fixture.request(OWN_TASK, &fixture.parent).await;
    let reply = fixture.request(OWN_TASK, &fixture.head).await;
    assert!(reply.contains(&format!("replacing queued {}", fixture.parent)), "{reply}");

    let runner = fixture.runner(BUILT);
    assert!(runner.run_next().unwrap());
    assert!(!runner.run_next().unwrap());
    let calls = fixture.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(calls[0].contains(&fixture.head), "{calls:?}");
}

#[tokio::test]
async fn failed_build_tells_the_worker_the_exact_error() {
    let fixture = Fixture::new();
    fixture.request(OWN_TASK, &fixture.head).await;
    fixture
        .runner("echo 'error[E0425]: exact compiler words' >&2; exit 3")
        .run_next()
        .unwrap();
    let messages = fixture.messages();
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert_eq!(messages[0].0, WORKER);
    assert!(messages[0].1.contains("FAILED"), "{}", messages[0].1);
    assert!(messages[0].1.contains("error[E0425]: exact compiler words"), "{}", messages[0].1);
    assert!(messages[0].1.contains("exit status: 3"), "{}", messages[0].1);
}

#[tokio::test]
async fn busy_machine_or_refused_admission_keeps_the_request_queued() {
    let fixture = Fixture::new();
    fixture.request(OWN_TASK, &fixture.head).await;
    let mut runner = fixture.runner(BUILT);

    let holder = hold_machine_build_lock().unwrap();
    assert!(!runner.run_next().unwrap(), "one build per machine");
    drop(holder);
    assert!(fixture.messages().is_empty());

    // A new MCP request arrives after the runner's read, during admission.
    runner.admit = |root| {
        let mut newer = read_request(&queued_path(root, OWN_TASK)).unwrap();
        newer.revision = git(&root.join("worktrees/robin"), &["rev-parse", "HEAD^"]);
        newer.requested_at += chrono::Duration::seconds(1);
        enqueue(root, &newer).unwrap();
        Err("disk below the watermark".to_owned())
    };
    assert!(!runner.run_next().unwrap());
    assert_eq!(read_request(&queued_path(&fixture.cas_root, OWN_TASK)).unwrap().revision, fixture.parent);
    runner.admit = |_| Err("disk below the watermark".to_owned());
    assert!(!runner.run_next().unwrap());
    assert!(!runner.run_next().unwrap());
    let messages = fixture.messages();
    assert_eq!(messages.len(), 1, "told once: {messages:?}");
    assert!(messages[0].1.contains("disk below the watermark"), "{}", messages[0].1);
    assert!(fixture.calls().is_empty());

    let runner = fixture.runner(BUILT);
    assert!(runner.run_next().unwrap(), "starts on its own once admitted");
    assert_eq!(fixture.calls().len(), 1);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn hung_build_kills_detached_children_releases_locks_and_reports_timeout() {
    let mut fixture = Fixture::new();
    let pids = fixture.temp.path().join("hung-pids");
    fixture._env.set("CANDIDATE_TEST_PIDS", &pids);
    fixture.request(OWN_TASK, &fixture.head).await;
    let mut runner = fixture.runner(r#"exec python3 - <<'PY'
import fcntl, os, signal, subprocess
from pathlib import Path
lock = open(Path(os.environ['HOME'])/'.cas/candidate-build.lock', 'w')
fcntl.flock(lock, fcntl.LOCK_EX)
child = subprocess.Popen(['python3','-c','import signal; signal.pause()'], start_new_session=True)
# Host PIDs remain observable via /proc even inside the build PID namespace.
def host_pid(pid):
    return int(Path(f'/proc/{pid}/status').read_text().split('NSpid:')[1].split()[0])
Path(os.environ['CANDIDATE_TEST_PIDS']).write_text(f'{host_pid(os.getpid())} {host_pid(child.pid)}')
signal.pause()
PY"#);
    // Advance the deadline only once the complete hung process tree exists.
    runner.build_elapsed = |_| {
        if Path::new(&std::env::var_os("CANDIDATE_TEST_PIDS").unwrap()).exists() {
            Duration::MAX
        } else { Duration::ZERO }
    };
    // Bound a broken runner as well: the red control must not leave a build behind.
    let (done, cancelled) = std::sync::mpsc::channel();
    let shutdown = runner.shutdown.clone();
    let watchdog = std::thread::spawn(move || {
        if cancelled.recv_timeout(Duration::from_secs(5)).is_err() {
            shutdown.store(true, Ordering::Relaxed);
        }
    });
    runner.run_next().unwrap();
    let _ = done.send(());
    watchdog.join().unwrap();
    let mut survivors = Vec::new();
    for pid in std::fs::read_to_string(&pids).unwrap().split_whitespace() {
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
        if !status.is_empty() && !status.lines().any(|line| line.starts_with("State:") && line.contains('Z')) {
            survivors.push(pid.to_owned());
            unsafe { libc::kill(pid.parse().unwrap(), libc::SIGKILL); }
        }
    }
    assert!(survivors.is_empty(), "surviving build PIDs: {survivors:?}");
    assert!(machine_lock_is_free().unwrap());
    let slot = open_lock(&fixture.cas_root.join("test-slot")).unwrap();
    assert!(slot.try_lock_exclusive().is_ok());
    assert!(!building_path(&fixture.cas_root, OWN_TASK).exists());
    let messages = fixture.messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].0, WORKER);
    assert!(messages[0].1.contains("timed out"), "{messages:?}");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn runner_heartbeats_while_fetch_is_waiting() {
    let mut fixture = Fixture::new();
    fixture.request(OWN_TASK, &fixture.head).await;
    let tools = fixture.temp.path().join("tools");
    std::fs::create_dir(&tools).unwrap();
    let real_git = String::from_utf8(Command::new("which").arg("git").output().unwrap().stdout).unwrap();
    let wrapper = tools.join("git");
    std::fs::write(&wrapper, format!(r#"#!/usr/bin/env python3
import ctypes, os, select, sys
from pathlib import Path
if 'fetch' in sys.argv:
    heartbeat = Path({heartbeat:?})
    libc = ctypes.CDLL(None)
    fd = libc.inotify_init1(os.O_CLOEXEC)
    assert fd >= 0
    assert libc.inotify_add_watch(fd, os.fsencode(heartbeat), 2) >= 0
    os.utime(heartbeat, (1, 1))
    if not select.select([fd], [], [], 5)[0]:
        Path({missed:?}).touch()
        sys.exit(1)
    os.close(fd)
os.execv({git:?}, [{git:?}] + sys.argv[1:])
"#, heartbeat=queue_dir(&fixture.cas_root).join(RUNNER_HEARTBEAT).to_str().unwrap(),
        missed=fixture.temp.path().join("missed-heartbeat").to_str().unwrap(), git=real_git.trim())).unwrap();
    std::fs::set_permissions(&wrapper, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let mut runner = fixture.runner(BUILT);
    runner.git_program = wrapper;
    runner.heartbeat_interval = Duration::ZERO;
    runner.run_next().unwrap();
    assert!(!fixture.temp.path().join("missed-heartbeat").exists(), "fetch received no heartbeat");
    assert!(runner_is_live(&fixture.cas_root));
}

#[tokio::test]
async fn a_dead_daemons_claim_is_built_by_the_next_one() {
    let fixture = Fixture::new();
    fixture.request(OWN_TASK, &fixture.head).await;
    std::fs::rename(
        queued_path(&fixture.cas_root, OWN_TASK),
        building_path(&fixture.cas_root, OWN_TASK),
    )
    .unwrap();
    assert!(fixture.runner(BUILT).run_next().unwrap());
    assert_eq!(fixture.calls().len(), 1);
    assert!(!building_path(&fixture.cas_root, OWN_TASK).exists());
    assert_eq!(fixture.messages().len(), 1);
}
