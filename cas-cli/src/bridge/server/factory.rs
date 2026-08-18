use anyhow::{Context, Result};

use crate::bridge::server::http::{error_response, json_response};
use crate::bridge::server::types::{StartFactoryRequest, StartFactoryResponse};

pub(crate) fn handle_factory_start(
    start: StartFactoryRequest,
    cors_allow_origin: Option<&str>,
) -> Result<tiny_http::Response<std::io::Cursor<Vec<u8>>>> {
    use std::process::{Command, Stdio};

    let project_dir = std::path::PathBuf::from(start.project_dir.clone());
    if !project_dir.exists() || !project_dir.is_dir() {
        return Ok(error_response(
            tiny_http::StatusCode(400),
            "invalid_project_dir",
            "project_dir must be an existing directory",
            cors_allow_origin,
        ));
    }

    // Default behavior: reuse an existing attachable session for this project if present.
    if start.reuse_existing {
        if let Ok(Some(existing)) = crate::ui::factory::SessionManager::new()
            .find_session_for_project(None, &project_dir.to_string_lossy())
        {
            if existing.can_attach() {
                let sj = crate::bridge::server::types::session_json(&existing);
                return Ok(json_response(
                    tiny_http::StatusCode(200),
                    &StartFactoryResponse {
                        schema_version: 1,
                        started: false,
                        reused_existing: true,
                        session: sj,
                    },
                    cors_allow_origin,
                ));
            }
        }
    }

    // Ensure Cassy is initialized in the project (create ./project/.cas if missing).
    if !project_dir.join(".cas").exists() {
        let exe = std::env::current_exe()?;
        let status = Command::new(&exe)
            .arg("--json")
            .arg("init")
            .arg("--yes")
            .current_dir(&project_dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .with_context(|| "Failed to run cas init")?;
        if !status.success() {
            return Ok(error_response(
                tiny_http::StatusCode(500),
                "init_failed",
                "cas init failed for project_dir",
                cors_allow_origin,
            ));
        }
    }

    let workers = start.workers.unwrap_or(0).min(6);
    let session_name = start.name.clone().unwrap_or_else(|| {
        crate::ui::factory::generate_session_name(Some(&project_dir.to_string_lossy()))
    });
    let supervisor_cli = start
        .supervisor_cli
        .clone()
        .unwrap_or_else(|| "claude".to_string());
    let worker_cli = start
        .worker_cli
        .clone()
        .unwrap_or_else(|| "claude".to_string());

    // Spawn the internal daemon command as a detached child process.
    // We use .spawn() (not .status()) because the daemon is long-running.
    let exe = std::env::current_exe()?;
    let mut cmd = Command::new(&exe);
    cmd.arg("factory")
        .arg("daemon")
        .arg("--session")
        .arg(&session_name)
        .arg("--cwd")
        .arg(&project_dir)
        .arg("--workers")
        .arg(workers.to_string())
        .arg("--supervisor-cli")
        .arg(&supervisor_cli)
        .arg("--worker-cli")
        .arg(&worker_cli)
        .current_dir(&project_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    if start.no_worktrees {
        cmd.arg("--no-worktrees");
    }
    if let Some(root) = start.worktree_root.as_ref().filter(|s| !s.is_empty()) {
        cmd.arg("--worktree-root").arg(root);
    }
    if start.notify {
        cmd.arg("--notify");
    }
    if start.tabbed {
        cmd.arg("--tabbed");
    }
    if start.record {
        cmd.arg("--record");
    }

    let _child = cmd
        .spawn()
        .with_context(|| "Failed to spawn factory daemon")?;

    // Wait briefly for session metadata + socket to appear.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut found: Option<crate::ui::factory::SessionInfo> = None;
    while std::time::Instant::now() < deadline {
        if let Ok(Some(s)) =
            crate::ui::factory::SessionManager::new().find_session(Some(&session_name))
        {
            if s.socket_exists && s.is_running {
                found = Some(s);
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    let Some(s) = found else {
        return Ok(error_response(
            tiny_http::StatusCode(504),
            "factory_start_timeout",
            "Timed out waiting for factory session to start",
            cors_allow_origin,
        ));
    };

    Ok(json_response(
        tiny_http::StatusCode(200),
        &StartFactoryResponse {
            schema_version: 1,
            started: true,
            reused_existing: false,
            session: crate::bridge::server::types::session_json(&s),
        },
        cors_allow_origin,
    ))
}
