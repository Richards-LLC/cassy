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
            "dry_run": if *dry_run { Some(true) } else if *no_dry_run { Some(false) } else { None },
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
    use clap::Parser;
    use std::time::Duration;

    #[test]
    fn gc_cli_reclaims_expired_artifacts_host_wide_cas_d740() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        crate::store::known_repos::ensure_host_schema().unwrap();
        let now = chrono::Utc::now();
        let mut roots = Vec::new();
        let mut expired = Vec::new();
        let mut kept = Vec::new();
        for name in ["one", "two"] {
            let project = env.home().join(name);
            let root = crate::store::init_cas_dir(&project).unwrap();
            std::fs::write(root.join("config.toml"), format!("[project]\ncanonical_id = {name:?}\n")).unwrap();
            crate::store::known_repos::register_repo_strict(&project).unwrap();
            let tasks = crate::store::open_task_store_local(&root).unwrap();
            let artifacts = crate::config::project_factory_artifacts_root(
                &root, &crate::config::resolved_factory_artifacts_root(None));
            for (suffix, age) in [("old", 30), ("fresh", 1), ("cited", 30), ("release", 30)] {
                let id = format!("cas-{name}-{suffix}");
                let mut task = cas_types::Task::new(id.clone(), id.clone());
                task.status = cas_types::TaskStatus::Closed;
                task.closed_at = Some(now - chrono::Duration::days(age));
                task.updated_at = task.closed_at.unwrap();
                tasks.add(&task).unwrap();
                let path = artifacts.join(&id);
                std::fs::create_dir_all(&path).unwrap();
                std::fs::write(path.join("proof.txt"), b"fixture proof").unwrap();
                let modified = std::time::SystemTime::now() - Duration::from_secs(age as u64 * 86400);
                for item in [path.join("proof.txt"), path.clone()] {
                    std::fs::File::open(item).unwrap().set_times(std::fs::FileTimes::new().set_modified(modified)).unwrap();
                }
                if suffix == "old" { expired.push(path); } else { kept.push(path); }
                if matches!(suffix, "cited" | "release") {
                    cas_store::SqliteArtifactStore::open(&root).unwrap().record_local(&cas_store::NewArtifact {
                        id: format!("art-{name}-{suffix}"), task_id: id, name: "proof.txt".into(),
                        mime: "text/plain".into(), size_bytes: 13, sha256: "a".repeat(64),
                    }).unwrap();
                }
            }
            let mut release = cas_store::NewQualityEvent::new(format!("release-{name}"), cas_store::QualityPayload::Release {
                commit_to_main_minutes: None, main_to_published_minutes: None, ejections: 0, rescues: 0,
            });
            release.local_note = Some(format!("art-{name}-release"));
            cas_store::record_quality_event(&root, &release).unwrap();
            roots.push(root);
        }
        // A live task in one project cites another project's published alias.
        let mut live = cas_types::Task::new("cas-live".into(), "Keep published proof".into());
        live.notes = "art-one-cited art-two-cited".into();
        crate::store::open_task_store_local(&roots[1]).unwrap().add(&live).unwrap();
        // Dead owner targets are disposable; unpushed source and live/cited
        // worktrees remain protected through the actual Git/reference probes.
        let git = |path: &Path, args: &[&str]| {
            let result = std::process::Command::new("git").arg("-C").arg(path)
                .args(args).env_remove("GIT_DIR").env_remove("GIT_WORK_TREE")
                .env_remove("GIT_INDEX_FILE").output().unwrap();
            assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        };
        let mut source_kept = Vec::new();
        for name in ["dead", "unpushed", "live", "cited"] {
            let path = roots[0].join("worktrees").join(name);
            std::fs::create_dir_all(&path).unwrap();
            git(&path, &["init", "-b", &format!("factory/{name}")]);
            std::fs::write(path.join(".gitignore"), "target/\n").unwrap();
            std::fs::write(path.join("source"), "committed").unwrap();
            git(&path, &["add", "."]);
            git(&path, &["-c", "user.name=Fixture", "-c", "user.email=fixture@example.test", "commit", "-m", "fixture"]);
            git(&path, &["update-ref", &format!("refs/remotes/origin/factory/{name}"), "HEAD"]);
            if name == "unpushed" {
                std::fs::write(path.join("source"), "local commit").unwrap();
                git(&path, &["add", "."]);
                git(&path, &["-c", "user.name=Fixture", "-c", "user.email=fixture@example.test", "commit", "-m", "unpushed"]);
            }
            let target = crate::factory_scratch::worker_target_dir(&path);
            std::fs::create_dir_all(&target).unwrap();
            std::fs::write(target.join("object"), "disposable").unwrap();
            if matches!(name, "live" | "cited") {
                let mut task = cas_types::Task::new(format!("cas-protect-{name}"), "Fixture protection".into());
                task.notes = format!("factory/{name}");
                crate::store::open_task_store_local(&roots[1]).unwrap().add(&task).unwrap();
                source_kept.push(path);
                kept.push(target);
            } else {
                expired.push(target);
                if name == "dead" { expired.push(path); } else { source_kept.push(path); }
            }
        }
        // Cross-project live citation through a terminal task must be transitive.
        let mut intermediate = cas_types::Task::new("cas-intermediary".into(), "Terminal reference".into());
        intermediate.status = cas_types::TaskStatus::Closed;
        intermediate.notes = "art-one-cited art-two-cited".into();
        crate::store::open_task_store_local(&roots[0]).unwrap().add(&intermediate).unwrap();
        let store = crate::store::open_task_store_local(&roots[1]).unwrap();
        live.notes = "cas-intermediary".into();
        store.update(&live).unwrap();
        for category in ["scratch", "review-scratch"] {
            for (name, old, cited) in [("expired", true, false), ("fresh", false, false), ("cited", true, true)] {
                let path = roots[0].join(category).join(name);
                std::fs::create_dir_all(&path).unwrap();
                std::fs::write(path.join("proof.txt"), "fixture proof").unwrap();
                if old {
                    let time = std::time::SystemTime::now() - Duration::from_secs(2 * 86400);
                    for item in [path.join("proof.txt"), path.clone()] {
                        std::fs::File::open(item).unwrap().set_times(std::fs::FileTimes::new().set_modified(time)).unwrap();
                    }
                }
                if cited {
                    let mut task = cas_types::Task::new(format!("cas-{category}-ref"), "Scratch reference".into());
                    task.notes = path.to_string_lossy().into_owned();
                    store.add(&task).unwrap();
                }
                if old && !cited { expired.push(path); } else { kept.push(path); }
            }
        }
        #[cfg(unix)]
        {
            let external = env.home().join("outside-retention");
            std::fs::create_dir_all(&external).unwrap();
            std::fs::write(external.join("proof.txt"), "must survive").unwrap();
            std::os::unix::fs::symlink(&external, roots[0].join("scratch/linked")).unwrap();
            // A forged persisted trash owner cannot authorize deletion outside
            // the registered category, even when the data itself is a symlink.
            let trash = env.home().join(".cas/retention/trash/forged");
            std::fs::create_dir_all(&trash).unwrap();
            std::fs::write(trash.join("owner.json"), serde_json::to_vec(&serde_json::json!({
                "project": roots[0], "category": "scratch", "path": external, "keys": []
            })).unwrap()).unwrap();
            std::os::unix::fs::symlink(&external, trash.join("data")).unwrap();
            kept.push(external);
        }
        env.set_current_dir(roots[0].parent().unwrap());
        env.set("CAS_ROOT", &roots[0]);
        let run = |extra: &[&str]| {
            let mut args = vec!["cas", "--json", "factory", "gc", "cleanup"];
            args.extend_from_slice(extra);
            let cli = crate::cli::Cli::try_parse_from(args).unwrap();
            let Some(crate::cli::Commands::Factory(factory)) = &cli.command else { unreachable!() };
            let Some(super::super::FactoryCommands::Gc { command }) = &factory.command else { unreachable!() };
            execute(command, &cli, Some(&roots[0])).unwrap();
        };
        run(&["--dry-run"]);
        assert!(expired.iter().all(|path| path.exists()));
        run(&[]);
        let report = crate::artifacts::retention::host::last_report().unwrap();
        assert!(report.reclaimed_bytes() > 0);
        assert!(!report.incomplete, "{:?}", report);
        for category in ["artifacts", "worktrees", "targets", "scratch", "review-scratch"] {
            assert!(report.rows.iter().any(|row| row.category == category && row.reclaimed_bytes > 0), "{category}: {report:?}");
        }
        assert!(source_kept.iter().all(|path| path.join("source").exists()));
        let reclaimed = expired.iter().filter(|path| !path.exists()).count() * 13;
        eprintln!("fixture: reclaimed_bytes={reclaimed}");
        assert!(expired.iter().all(|path| !path.exists()));
        assert!(kept.iter().all(|path| path.exists()), "{kept:?}");
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
