//! CLI entry points for the factory GC MCP actions.

use std::io::Write;
use std::path::Path;

use anyhow::{Result, bail};
use rmcp::handler::server::wrapper::Parameters;
use serde_json::json;

use super::FactoryGcCommands;

pub(super) fn execute(
    command: &FactoryGcCommands,
    cli: &crate::cli::Cli,
    cas_root: Option<&Path>,
) -> Result<()> {
    let cas_root =
        cas_root.ok_or_else(|| anyhow::anyhow!("factory GC requires a Cassy project"))?;

    let request = match command {
        FactoryGcCommands::Report => {
            json!({ "action": "gc_report", "json": cli.json, "full": cli.full })
        }
        FactoryGcCommands::Cleanup {
            force,
            dry_run,
            no_dry_run,
        } => json!({
            "action": "gc_cleanup",
            "json": cli.json,
            "full": cli.full,
            "force": force,
            // dry_run gates only the stricter cache/process/journey tiers;
            // earlier metadata cleanup and force-only tiers keep their gates.
            "dry_run": *dry_run || !*no_dry_run,
        }),
    };
    let request =
        serde_json::from_value::<crate::mcp::tools::service::CoordinationRequest>(request)?;

    let core = crate::mcp::CasCore::with_daemon(cas_root.to_path_buf(), None, None);
    #[cfg(feature = "mcp-proxy")]
    let service = crate::mcp::CasService::new(core, None);
    #[cfg(not(feature = "mcp-proxy"))]
    let service = crate::mcp::CasService::new(core);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let response = with_cleanup_progress(
        matches!(command, FactoryGcCommands::Cleanup { .. }) && !cli.json,
        std::time::Duration::from_secs(5),
        |elapsed| {
            eprintln!("[INFO] GC cleanup running: {elapsed}s elapsed; inventory can take time.")
        },
        || runtime.block_on(service.factory(Parameters(request))),
    )
    .map_err(|error| anyhow::Error::msg(error.message.to_string()))?;

    let mut stdout = std::io::stdout().lock();
    let mut wrote_text = false;
    for content in response.content {
        if let Some(text) = content.as_text() {
            stdout.write_all(text.text.as_bytes())?;
            wrote_text = true;
        }
    }
    if !wrote_text {
        bail!("factory GC handler returned no text report");
    }
    Ok(())
}

/// Inventory contains blocking filesystem/process probes, so the heartbeat
/// must not depend on the handler yielding to the current-thread runtime.
fn with_cleanup_progress<T>(
    enabled: bool,
    interval: std::time::Duration,
    mut report: impl FnMut(u64) + Send,
    operation: impl FnOnce() -> T,
) -> T {
    if !enabled {
        return operation();
    }
    std::thread::scope(|scope| {
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let heartbeat = scope.spawn(move || {
            let started = std::time::Instant::now();
            loop {
                report(started.elapsed().as_secs());
                if stopped.recv_timeout(interval) != Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                {
                    break;
                }
            }
        });
        let result = operation();
        drop(stop);
        let _ = heartbeat.join();
        result
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn gc_cleanup_reclaims_expired_host_artifacts_cas_f1b7() {
        use cas_store::TaskStore;
        let env = crate::test_support::TestEnvGuard::temp_home();
        crate::store::known_repos::ensure_host_schema().unwrap();
        let mut expired = Vec::new();
        let mut kept = Vec::new();
        let mut roots = Vec::new();
        for name in ["one", "two"] {
            let project = env.home().join(name);
            let root = crate::store::init_cas_dir(&project).unwrap();
            std::fs::write(root.join("config.toml"), format!("[project]\ncanonical_id = {name:?}\n")).unwrap();
            crate::store::known_repos::register_repo_strict(&project).unwrap();
            for (suffix, closed) in [("old", true), ("live", false)] {
                let mut task = cas_types::Task::new(format!("cas-{name}-{suffix}"), "Retention fixture".into());
                if closed {
                    task.status = cas_types::TaskStatus::Closed;
                    task.closed_at = Some(chrono::Utc::now() - chrono::Duration::days(30));
                    task.updated_at = task.closed_at.unwrap();
                }
                cas_store::SqliteTaskStore::open(&root).unwrap().add(&task).unwrap();
                let base = crate::config::resolved_factory_artifacts_root(None);
                let path = crate::config::project_factory_artifacts_root(&root, &base).join(&task.id);
                std::fs::create_dir_all(&path).unwrap();
                std::fs::write(path.join("proof.txt"), b"fixture evidence").unwrap();
                let old = std::time::SystemTime::now() - Duration::from_secs(30 * 86400);
                for item in [path.join("proof.txt"), path.clone()] {
                    std::fs::File::open(item).unwrap().set_times(std::fs::FileTimes::new().set_modified(old)).unwrap();
                }
                if closed { expired.push(path); } else { kept.push(path); }
            }
            roots.push(root);
        }
        let cli: crate::cli::Cli = clap::Parser::try_parse_from(["cas", "factory", "gc", "cleanup"]).unwrap();
        let command = FactoryGcCommands::Cleanup { force: false, dry_run: false, no_dry_run: false };
        execute(&command, &cli, Some(&roots[0])).unwrap();
        assert!(expired.iter().all(|path| !path.exists()), "expired artifacts remain: {expired:?}");
        assert!(kept.iter().all(|path| path.join("proof.txt").exists()));
    }

    #[test]
    fn cleanup_progress_runs_during_blocking_work_and_is_silent_when_disabled() {
        let (lines, received) = std::sync::mpsc::channel();
        with_cleanup_progress(
            true,
            Duration::from_millis(1),
            |elapsed| {
                lines.send(elapsed).unwrap();
            },
            || {
                // The operation cannot finish until both the initial update and a
                // heartbeat arrive; no sleep or async scheduling is involved.
                for _ in 0..2 {
                    received.recv_timeout(Duration::from_secs(5)).unwrap();
                }
            },
        );
        assert_eq!(
            with_cleanup_progress(
                false,
                Duration::ZERO,
                |_| panic!("JSON/report mode must never start progress"),
                || 7
            ),
            7
        );
    }
}
