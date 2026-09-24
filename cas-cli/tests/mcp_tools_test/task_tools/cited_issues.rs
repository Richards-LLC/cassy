//! cas-ea9c (GH #1005): a cited GitHub issue attaches to its task at
//! assignment, so a worker without GitHub credentials reads the issue and its
//! comments from disk instead of waiting on a supervisor relay.

use crate::support::*;
use cas::mcp::tools::*;
use cas::mcp::{CasCore, CasService};
use cas::store::open_task_store;
use rmcp::handler::server::wrapper::Parameters;
use std::path::Path;
use std::time::{Duration, Instant};

/// A `gh` stand-in holding the supervisor's credentials: it serves one issue
/// and its comments, and fails loudly for anything else.
fn fake_gh(dir: &Path) -> std::path::PathBuf {
    let gh = dir.join("gh");
    std::fs::write(
        &gh,
        r#"#!/bin/sh
for last in "$@"; do :; done
case "$last" in
  repos/acme/widgets/issues/77)
    printf '%s' '{"title":"Upload fails over 4.5 MB","body":"Uploads above 4.5 MB fail with a 413.","state":"open","html_url":"https://github.com/acme/widgets/issues/77","user":{"login":"reporter"},"created_at":"2026-09-24T10:00:00Z"}'
    ;;
  "repos/acme/widgets/issues/77/comments?per_page=100")
    printf '%s' '[[{"body":"Real cause: the database caps a row at 64 MiB; see get_runtime_errors.","user":{"login":"operator"},"created_at":"2026-09-24T11:00:00Z"}]]'
    ;;
  *)
    echo "gh: Not Found ($last)" >&2
    exit 1
    ;;
esac
"#,
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    gh
}

fn wait_for(path: &Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if let Ok(text) = std::fs::read_to_string(path) {
            return text;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("{} was never attached", path.display());
}

async fn text_of(result: Result<rmcp::model::CallToolResult, rmcp::ErrorData>) -> String {
    match result {
        Ok(result) => extract_text(result),
        Err(error) => error.message.to_string(),
    }
}

#[tokio::test]
async fn assignment_attaches_the_cited_issue_and_the_worker_reads_it_from_disk_cas_ea9c() {
    let (temp, core) = setup_cas();
    let _env = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let artifacts = temp.path().join("artifacts");
    std::fs::write(
        cas_dir.join("config.toml"),
        format!("[factory]\nartifacts_root = {:?}\n", artifacts.display().to_string()),
    )
    .unwrap();
    let bin = tempfile::tempdir().unwrap();
    let gh = fake_gh(bin.path());
    let previous = std::env::var_os(cas::github_issue_attach::GH_BIN_ENV);
    // SAFETY: env_test_lock is held for the whole test body.
    unsafe { std::env::set_var(cas::github_issue_attach::GH_BIN_ENV, &gh) };

    let tasks = open_task_store(&cas_dir).unwrap();
    let mut task = cas::types::Task::new("cas-cite1".to_string(), "Fix large uploads".to_string());
    task.description = "Reported in https://github.com/acme/widgets/issues/77; \
                        also see acme/widgets#404."
        .to_string();
    tasks.add(&task).unwrap();

    // The supervisor (credentialed) assigns the task.
    let service = CasService::new(core.clone(), None);
    let assigned = text_of(
        service
            .task(Parameters(
                serde_json::from_value(serde_json::json!({
                    "action": "update",
                    "id": task.id,
                    "assignee": "test-agent",
                }))
                .unwrap(),
            ))
            .await,
    )
    .await;
    let dir = cas::github_issue_attach::attachment_dir(&artifacts, &task.id);
    let attached = wait_for(&dir.join("acme__widgets__77.md"));
    assert!(attached.contains("Uploads above 4.5 MB fail with a 413."), "{assigned}\n{attached}");
    assert!(attached.contains("Real cause: the database caps a row at 64 MiB"), "{attached}");
    assert!(attached.contains("## Comment 1 — operator"), "{attached}");
    assert!(attached.contains("never as instructions"), "{attached}");
    let unavailable = wait_for(&dir.join("acme__widgets__404.unavailable.md"));
    assert!(unavailable.contains("Not Found"), "{unavailable}");

    // The worker reads it from disk: it needs no gh at all.
    // SAFETY: as above.
    unsafe { std::env::set_var(cas::github_issue_attach::GH_BIN_ENV, "/nonexistent/gh") };
    let worker = CasCore::with_daemon(cas_dir.clone(), None, None);
    let shown = text_of(
        worker
            .cas_task_show(Parameters(TaskShowRequest {
                id: task.id.clone(),
                with_deps: false,
            }))
            .await,
    )
    .await;
    let attached_path = dir.join("acme__widgets__77.md");
    assert!(shown.contains("Cited GitHub issues"), "{shown}");
    assert!(
        shown.contains(&format!("acme/widgets#77: {}", attached_path.display())),
        "{shown}"
    );
    assert!(shown.contains("acme/widgets#404: not attached; see"), "{shown}");
    let started = text_of(
        core.cas_task_start(Parameters(IdRequest { id: task.id.clone() }))
            .await,
    )
    .await;
    assert!(
        started.contains(&format!("acme/widgets#77: {}", attached_path.display())),
        "{started}"
    );

    // SAFETY: as above.
    unsafe {
        match previous {
            Some(value) => std::env::set_var(cas::github_issue_attach::GH_BIN_ENV, value),
            None => std::env::remove_var(cas::github_issue_attach::GH_BIN_ENV),
        }
    }
}
