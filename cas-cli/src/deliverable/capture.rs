//! One bounded QA run over immutable merge-base/head checkouts.
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use super::asset::{Asset, Workspace};
use super::pairs::{self, Pair, PairManifest, SideCapture};
use super::{Finding, Receipt, UsageError};
use crate::bounded_process::{self, BoundedCommandError, Deadline};
use crate::config::{Config, QaCaptureConfig};
use serde::Serialize;
use serde_json::{Value, json};

const SCRIPT: Asset = Asset {
    path: "capture.mjs",
    content: include_str!("assets/capture.mjs"),
};
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Side {
    Both,
    Before,
    After,
}
#[derive(Debug, Clone)]
pub struct CaptureRequest {
    pub root: PathBuf,
    pub cas_root: PathBuf,
    pub task: String,
    pub side: Side,
    pub dir: Option<PathBuf>,
    pub target: Option<String>,
    pub shots: Option<PathBuf>,
}
#[derive(Debug, Serialize)]
pub struct CaptureReceipt {
    pub schema: &'static str,
    pub task_id: String,
    pub base_sha: String,
    pub head_sha: String,
    pub manifest: String,
    pub pairs_status: String,
    pub pairs: usize,
    pub unpaired: usize,
    pub elapsed_ms: u64,
    pub warnings: Vec<String>,
    pub findings: Vec<Finding>,
}
impl Receipt for CaptureReceipt {
    fn findings(&self) -> &[Finding] {
        &self.findings
    }
    fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

pub fn capture(request: &CaptureRequest) -> anyhow::Result<CaptureReceipt> {
    let started = Instant::now();
    if !pairs::valid_id(&request.task) {
        return Err(UsageError("--task requires a task identifier".into()).into());
    }
    let config = Config::load(&request.cas_root)?;
    let mut capture = config.qa().capture.unwrap_or_default();
    if let Some(path) = &request.shots {
        capture.shots = path.to_string_lossy().into();
    }
    capture.validate().map_err(UsageError)?;
    let credentials = capture
        .credential_file()
        .map_err(UsageError)?
        .map(std::path::absolute)
        .transpose()?;
    let deadline = Deadline::after(
        Duration::from_secs(capture.timeout_secs.into()).saturating_sub(started.elapsed()),
    );
    let head = git(
        &request.root,
        &["rev-parse", "--verify", "HEAD^{commit}"],
        deadline,
    )?;
    let target = if let Some(target) = &request.target {
        target.clone()
    } else {
        let tasks = crate::store::open_task_store(&request.cas_root)?;
        let task = tasks.get(&request.task).map_err(|_| {
            UsageError("task is unavailable; use its project root or an explicit --target".into())
        })?;
        task.deliverables
            .work_target
            .map(|target| target.target_branch)
            .ok_or_else(|| {
                UsageError("task has no declared target; pass --target explicitly".into())
            })?
    };
    let target = git(
        &request.root,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{target}^{{commit}}"),
        ],
        deadline,
    )?;
    let base = git(&request.root, &["merge-base", &target, &head], deadline)?;
    let dir = request
        .dir
        .clone()
        .unwrap_or_else(|| pairs::task_dir(&config, &request.cas_root, &request.task));
    let mut receipt = CaptureReceipt {
        schema: "cas.deliverable.capture/1",
        task_id: request.task.clone(),
        base_sha: base.clone(),
        head_sha: head.clone(),
        manifest: dir.join("pairs.json").display().to_string(),
        pairs_status: "unavailable".into(),
        pairs: 0,
        unpaired: 0,
        elapsed_ms: 0,
        warnings: vec![],
        findings: vec![],
    };
    fs::create_dir_all(&dir)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(".capture.lock"))?;
    fs2::FileExt::try_lock_exclusive(&lock).map_err(|_| {
        UsageError("capture is already writing this task's pairs; wait for its receipt".into())
    })?;
    let old = pairs::read_manifest(&dir.join("pairs.json"))
        .ok()
        .filter(|m| m.schema == 1 && m.task_id == request.task);
    let mut manifest = PairManifest {
        schema: 1,
        task_id: request.task.clone(),
        base_sha: base.clone(),
        head_sha: head.clone(),
        origin: capture.base_url.clone(),
        pairs: vec![],
        unpaired: vec![],
        findings: vec![pairs::finding(
            "capture-incomplete",
            "Capture has not completed",
        )],
    };
    pairs::write_manifest(&dir, &manifest)?;
    let catalog = git_optional(
        &request.root,
        &["show", &format!("{head}:{}", capture.shots)],
        deadline,
    )?;
    let Some(catalog) = catalog else {
        return journeys::capture(
            request, &config, &capture, &base, &head, &dir, deadline, started, receipt,
        );
    };
    if catalog.len() > 1024 * 1024 {
        return Err(UsageError("shot catalog exceeds 1 MB".into()).into());
    }
    let mut shots: Vec<Value> = serde_json::from_str(&catalog)
        .map_err(|_| UsageError("shot catalog must be a JSON array".into()))?;
    let changed = git(
        &request.root,
        &["diff", "--name-only", &base, &head],
        deadline,
    )?;
    if request.side == Side::Both {
        shots.retain(|shot| {
            shot.get("touches")
                .and_then(Value::as_array)
                .is_none_or(|globs| {
                    changed.lines().any(|path| {
                        crate::qa_registry::matches_path(
                            globs.iter().filter_map(Value::as_str),
                            path,
                        )
                    })
                })
        });
    }
    if shots.is_empty() {
        receipt.findings.push(pairs::finding("capture-unavailable","No catalog shots match this delivery; declare their touches or affected journey receipts"));
        manifest.findings = receipt.findings.clone();
        pairs::write_manifest(&dir, &manifest)?;
        receipt.elapsed_ms = started.elapsed().as_millis() as u64;
        return Ok(receipt);
    }
    let workspace = Workspace::materialize(&[SCRIPT])?;
    let plan_path = workspace.path("plan.json");
    let mut plan = json!({"root":request.root,"config":capture,"credentials_file":credentials,"shots":shots,"out":dir,"phases":[]});
    fs::write(&plan_path, serde_json::to_vec(&plan)?)?;
    let checked = node(&workspace, &plan_path, true, &request.root, deadline)?;
    if let Some(error) = checked.get("code").and_then(Value::as_str) {
        return Err(UsageError(format!(
            "{error}: invalid shot catalog; check IDs, paths, selectors and budget"
        ))
        .into());
    }
    if capture.serve.is_empty() && request.side == Side::Both {
        receipt.findings.push(pairs::finding("capture-unavailable","Merge-base capture requires qa.capture.serve; collect before/after explicitly for a deployed target"));
        manifest.findings = receipt.findings.clone();
        pairs::write_manifest(&dir, &manifest)?;
        receipt.elapsed_ms = started.elapsed().as_millis() as u64;
        return Ok(receipt);
    }
    let result = (|| -> anyhow::Result<Value> {
        let staging_root = request.cas_root.join("tmp");
        fs::create_dir_all(&staging_root)?;
        let staging = tempfile::Builder::new()
            .prefix("capture-")
            .tempdir_in(&staging_root)?;
        let mut servers = Vec::new();
        let mut phases = Vec::new();
        if !capture.serve.is_empty() {
            let changed_dependencies = git(
                &request.root,
                &[
                    "diff",
                    "--name-only",
                    &base,
                    &head,
                    "--",
                    "**/package-lock.json",
                    "**/pnpm-lock.yaml",
                    "**/yarn.lock",
                    "package-lock.json",
                    "pnpm-lock.yaml",
                    "yarn.lock",
                ],
                deadline,
            )?;
            if !changed_dependencies.is_empty() {
                anyhow::bail!("capture-dependencies");
            }
        }
        for (side, reference) in [("before", &base), ("after", &head)] {
            if (request.side == Side::Before && side == "after")
                || (request.side == Side::After && side == "before")
            {
                continue;
            }
            if capture.serve.is_empty() {
                if capture.base_url.is_empty() {
                    return Err(
                        UsageError("capture requires qa.capture.serve or base_url".into()).into(),
                    );
                }
                phases.push(json!({"side":side,"ref":capture.base_url,"source":"deployment","root":request.root,"base":capture.base_url}));
            } else {
                let root = staging.path().join(side);
                let _ = git(
                    &request.root,
                    &[
                        "clone",
                        "--local",
                        "--no-checkout",
                        "--quiet",
                        &request.root.to_string_lossy(),
                        &root.to_string_lossy(),
                    ],
                    deadline,
                )?;
                let hooks = staging.path().join("hooks");
                fs::create_dir_all(&hooks)?;
                let _ = git(
                    &root,
                    &[
                        "-c",
                        &format!("core.hooksPath={}", hooks.display()),
                        "checkout",
                        "--quiet",
                        "--detach",
                        reference,
                    ],
                    deadline,
                )?;
                reuse_dependencies(&request.root, &root, deadline)?;
                let server = Server::start(
                    &root,
                    &capture,
                    &staging_root,
                    staging.path(),
                    side,
                    deadline,
                )?;
                let origin = format!("http://127.0.0.1:{}", server.port);
                if manifest.origin.is_empty() {
                    manifest.origin = origin.clone();
                }
                phases.push(json!({"side":side,"ref":reference,"source":if side=="before"{"merge-base"}else{"head"},"root":root,"base":origin}));
                servers.push(server);
            }
        }
        plan["phases"] = json!(phases);
        plan["remaining_ms"] = json!(deadline.remaining().as_millis() as u64);
        fs::write(&plan_path, serde_json::to_vec(&plan)?)?;
        let result = node(&workspace, &plan_path, false, &request.root, deadline);
        drop(servers);
        drop(staging);
        result
    })();
    let result = match result {
        Ok(value) => value,
        Err(error) => {
            if error.downcast_ref::<UsageError>().is_some() {
                return Err(error);
            }
            let code = if deadline.remaining().is_zero() {
                "capture-timeout"
            } else if error.to_string() == "capture-dependencies" {
                "capture-dependencies"
            } else {
                "capture-server"
            };
            receipt.findings.push(pairs::finding(code,if code=="capture-dependencies"{"Dependency lockfiles differ; cannot reuse delivered node_modules for the base"}else{"Capture could not complete within its budget; verify serve command and readiness"}));
            json!({})
        }
    };
    if let Some(code) = result.get("code").and_then(Value::as_str) {
        receipt.findings.push(pairs::finding(
            code,
            result["message"].as_str().unwrap_or("Capture unavailable"),
        ));
    }
    let mut cells = BTreeMap::<String, Pair>::new();
    if request.side != Side::Both {
        if let Some(old) = old {
            for mut cell in old.pairs.into_iter().chain(old.unpaired) {
                if request.side == Side::Before {
                    cell.before = None;
                } else {
                    cell.after = None;
                }
                cell.reason = None;
                cells.insert(cell.id.clone(), cell);
            }
        }
    }
    if let Some(captures) = result["captures"].as_array() {
        for value in captures {
            let id = value["id"]
                .as_str()
                .ok_or_else(|| UsageError("capture returned an invalid identity".into()))?;
            let framing: [u32; 2] = serde_json::from_value(value["viewport"].clone())?;
            let masks: Vec<String> = serde_json::from_value(value["masked"].clone())?;
            let scheme = value["scheme"].as_str().unwrap_or("light");
            let cell = cells.entry(id.into()).or_insert_with(|| Pair {
                id: id.into(),
                surface: value["surface"].as_str().unwrap_or(id).into(),
                viewport: framing,
                scheme: scheme.into(),
                masked: masks.clone(),
                before: None,
                after: None,
                reason: None,
            });
            if cell.viewport != framing || cell.scheme != scheme || cell.masked != masks {
                cell.before = None;
                cell.after = None;
                cell.reason = Some("Framing or masks changed between captures".into());
            }
            let side: SideCapture = serde_json::from_value(value["capture"].clone())?;
            if value["side"] == "before" {
                cell.before = Some(side);
            } else {
                cell.after = Some(side);
            }
        }
    }
    for (_, mut cell) in cells {
        if cell.before.is_some() && cell.after.is_some() && cell.reason.is_none() {
            manifest.pairs.push(cell);
        } else {
            cell.reason
                .get_or_insert_with(|| "One capture side is unavailable".into());
            manifest.unpaired.push(cell);
        }
    }
    if let Some(failures) = result["failures"].as_array() {
        for failure in failures {
            receipt.findings.push(pairs::finding(
                failure["code"].as_str().unwrap_or("capture-shot"),
                format!(
                    "{}: {}",
                    failure["id"].as_str().unwrap_or("shot"),
                    failure["message"].as_str().unwrap_or("Shot unavailable")
                ),
            ));
        }
    }
    receipt.pairs = manifest.pairs.len();
    receipt.unpaired = manifest.unpaired.len();
    receipt.pairs_status =
        if receipt.pairs > 0 && receipt.findings.is_empty() && receipt.unpaired == 0 {
            "complete"
        } else if receipt.pairs > 0 {
            "partial"
        } else {
            "unavailable"
        }
        .into();
    if request.side == Side::Both && receipt.pairs == 0 && receipt.findings.is_empty() {
        receipt.findings.push(pairs::finding(
            "unpaired-capture",
            "No complete before/after pair; collect the missing side",
        ));
    }
    if request.side != Side::Both && receipt.pairs == 0 && receipt.unpaired > 0 {
        receipt.pairs_status = "partial".into();
        receipt
            .warnings
            .push("One side collected; pairs excludes it until the other side is captured".into());
    }
    manifest.findings = receipt.findings.clone();
    pairs::write_manifest(&dir, &manifest)?;
    receipt.elapsed_ms = started.elapsed().as_millis() as u64;
    Ok(receipt)
}

fn node(
    workspace: &Workspace,
    plan: &Path,
    validate: bool,
    root: &Path,
    deadline: Deadline,
) -> anyhow::Result<Value> {
    let mut command = Command::new("node");
    command
        .current_dir(root)
        .arg(workspace.path(SCRIPT.path))
        .arg(plan);
    if validate {
        command.arg("--validate");
    }
    let output = bounded_process::run_command(&mut command, deadline, deadline.remaining())
        .map_err(|error| match error {
            BoundedCommandError::Io => {
                UsageError("Node is unavailable; install node to capture evidence".into()).into()
            }
            BoundedCommandError::TimedOut => anyhow::anyhow!("capture-timeout"),
        })?;
    let result: Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| UsageError("capture runtime returned no readable receipt".into()))?;
    if output.status.code() == Some(2) {
        return Err(UsageError(
            result["message"]
                .as_str()
                .unwrap_or("capture runtime unavailable")
                .into(),
        )
        .into());
    }
    Ok(result)
}
fn git(root: &Path, args: &[&str], deadline: Deadline) -> anyhow::Result<String> {
    git_optional(root, args, deadline)?.ok_or_else(|| {
        UsageError("Git cannot resolve or prepare the capture revisions".into()).into()
    })
}
fn git_optional(root: &Path, args: &[&str], deadline: Deadline) -> anyhow::Result<Option<String>> {
    let output = bounded_process::run_command(
        Command::new("git").current_dir(root).args(args),
        deadline,
        Duration::from_secs(30),
    )
    .map_err(|_| UsageError("Git is unavailable or the capture budget expired".into()))?;
    Ok(output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().into()))
}
fn reuse_dependencies(source: &Path, target: &Path, deadline: Deadline) -> anyhow::Result<()> {
    // Only dependency directories are shared; cloned source files stay private.
    let mut entries = walkdir::WalkDir::new(source)
        .max_depth(4)
        .into_iter()
        .filter_entry(|e| {
            !matches!(
                e.file_name().to_str(),
                Some(".git" | ".cas" | "target" | "dist")
            ) && !e
                .path()
                .strip_prefix(source)
                .ok()
                .and_then(Path::parent)
                .is_some_and(|p| p.components().any(|c| c.as_os_str() == "node_modules"))
        });
    while let Some(entry) = entries.next() {
        if deadline.remaining().is_zero() {
            anyhow::bail!("capture-timeout");
        }
        let entry = entry?;
        if entry.file_name() != "node_modules" || !entry.path().is_dir() {
            continue;
        }
        let relative = entry.path().strip_prefix(source)?;
        if relative
            .parent()
            .is_some_and(|p| p.components().any(|c| c.as_os_str() == "node_modules"))
        {
            continue;
        }
        let destination = target.join(relative);
        if destination.exists() {
            continue;
        }
        if let Some(parent) = destination.parent().filter(|p| p.is_dir()) {
            let _ = parent;
            #[cfg(unix)]
            std::os::unix::fs::symlink(entry.path(), &destination)?;
            // A directory-only node_modules/ ignore does not match a symlink.
            // Keep dependency reuse out of exact-tip producer dirty checks,
            // without changing the evaluated checkout's tracked ignore file.
            OpenOptions::new()
                .append(true)
                .create(true)
                .open(target.join(".git/info/exclude"))?
                .write_all(b"\nnode_modules\n")?;
            #[cfg(not(unix))]
            {
                return Err(UsageError(
                    "node_modules reuse currently requires Unix symlinks".into(),
                )
                .into());
            }
        }
    }
    Ok(())
}

struct Server {
    child: Child,
    port: u16,
    _lease: File,
}
impl Server {
    fn start(
        root: &Path,
        config: &QaCaptureConfig,
        leases: &Path,
        logs: &Path,
        side: &str,
        deadline: Deadline,
    ) -> anyhow::Result<Self> {
        let (port, lease) = lease_port(leases)?;
        let args = shell_words::split(&config.serve.replace("{port}", &port.to_string()))
            .map_err(|_| UsageError("qa.capture.serve has invalid quoting".into()))?;
        let (program, args) = args
            .split_first()
            .ok_or_else(|| UsageError("qa.capture.serve is empty".into()))?;
        let log = File::create(logs.join(format!("{side}.server.log")))?;
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(root)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        bounded_process::configure_process_group(&mut command);
        let child = command
            .spawn()
            .map_err(|_| anyhow::anyhow!("capture-server"))?;
        let mut server = Self {
            child,
            port,
            _lease: lease,
        };
        while !deadline.remaining().is_zero() {
            if server.child.try_wait()?.is_some() {
                anyhow::bail!("capture-server");
            }
            if let Ok(mut stream) = TcpStream::connect_timeout(
                &([127, 0, 0, 1], port).into(),
                deadline.remaining().min(Duration::from_millis(250)),
            ) {
                let timeout = Some(
                    deadline
                        .remaining()
                        .min(Duration::from_millis(250))
                        .max(Duration::from_millis(1)),
                );
                stream.set_read_timeout(timeout)?;
                stream.set_write_timeout(timeout)?;
                if write!(
                    stream,
                    "GET {} HTTP/1.0\r\nHost: localhost\r\n\r\n",
                    config.ready_path
                )
                .is_ok()
                {
                    let mut line = String::new();
                    if BufReader::new(stream).read_line(&mut line).is_ok()
                        && (line.starts_with("HTTP/1.0 2")
                            || line.starts_with("HTTP/1.1 2")
                            || line.starts_with("HTTP/1.0 3")
                            || line.starts_with("HTTP/1.1 3"))
                    {
                        return Ok(server);
                    }
                }
            }
            std::thread::sleep(deadline.remaining().min(Duration::from_millis(20)));
        }
        anyhow::bail!("capture-timeout")
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        bounded_process::terminate_process_group(&mut self.child);
    }
}
fn lease_port(root: &Path) -> anyhow::Result<(u16, File)> {
    fs::create_dir_all(root.join("capture-ports"))?;
    for _ in 0..32 {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join("capture-ports").join(format!("{port}.lock")))?;
        if fs2::FileExt::try_lock_exclusive(&file).is_ok() {
            drop(listener);
            return Ok((port, file));
        }
    }
    Err(UsageError("No capture port lease is available".into()).into())
}

mod journeys;
