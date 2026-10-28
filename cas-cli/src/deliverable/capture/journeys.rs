//! Goal-stage receipts, never a journey's entry page.
use super::*;
use anyhow::Context;

const INSTRUMENTATION: &[&str] = &[
    "scripts/journey-eval.sh",
    "scripts/journey-receipt.py",
    "hub-web/e2e/journeys/journey.ts",
    "hub-web/e2e/journeys/receipt-directory.mjs",
    "hub-web/e2e/journeys/receipt-directory.d.mts",
];

// The producer owns selection and assertions. The private checkouts evaluate
// each revision's application, committed dist and tests with head instrumentation.
pub(super) fn capture(
    request: &CaptureRequest,
    _config: &Config,
    capture: &QaCaptureConfig,
    base: &str,
    head: &str,
    dir: &Path,
    deadline: Deadline,
    started: Instant,
    mut receipt: CaptureReceipt,
) -> anyhow::Result<CaptureReceipt> {
    let mut manifest = PairManifest {
        schema: 1,
        task_id: request.task.clone(),
        base_sha: base.into(),
        head_sha: head.into(),
        origin: "journey:committed-dist/protocol-double".into(),
        pairs: vec![],
        unpaired: vec![],
        findings: vec![],
    };
    let result = run(request, capture, base, head, dir, deadline, &mut manifest);
    if let Err(error) = result {
        let code = if deadline.remaining().is_zero() {
            "capture-timeout"
        } else {
            "capture-unavailable"
        };
        // Process output is private: it can include application-controlled text.
        receipt.findings.push(if code == "capture-timeout" {
            timeout_finding(capture.timeout_secs, started)
        } else {
            pairs::finding(code, if let Some(usage) = error.downcast_ref::<UsageError>() {
                usage.0.clone()
            } else { "Goal-stage journey capture could not complete; inspect its retained receipt or configure qa.capture.shots".into() })
        });
    }
    receipt.pairs = manifest.pairs.len();
    receipt.unpaired = manifest.unpaired.len();
    if receipt.unpaired > 0 {
        receipt.findings.push(pairs::finding(
            "unpaired-capture",
            "Both journey runs must pass with matching final-stage framing and masks",
        ));
    }
    if receipt.pairs == 0 && receipt.findings.is_empty() {
        receipt.findings.push(pairs::finding(
            "capture-unavailable",
            "No affected final-stage journey pairs; configure qa.capture.shots",
        ));
    }
    receipt.pairs_status = if receipt.pairs > 0 && receipt.findings.is_empty() {
        "complete"
    } else if receipt.pairs > 0 {
        "partial"
    } else {
        "unavailable"
    }
    .into();
    manifest.findings = receipt.findings.clone();
    pairs::write_manifest(dir, &manifest)?;
    receipt.elapsed_ms = started.elapsed().as_millis() as u64;
    Ok(receipt)
}

fn run(
    request: &CaptureRequest,
    config: &QaCaptureConfig,
    base: &str,
    head: &str,
    dir: &Path,
    deadline: Deadline,
    manifest: &mut PairManifest,
) -> anyhow::Result<()> {
    if request.side != Side::Both {
        return Err(UsageError("Journey capture requires --side both; use a shot catalog for deployed one-side capture".into()).into());
    }
    for path in INSTRUMENTATION {
        if git_optional(
            &request.root,
            &["show", &format!("{head}:{path}")],
            deadline,
        )?
        .is_none()
        {
            return Err(UsageError("No shot catalog or comparison-capable journey producer; configure qa.capture.shots".into()).into());
        }
    }
    let dependencies = git(
        &request.root,
        &[
            "diff",
            "--name-only",
            base,
            head,
            "--",
            "hub-web/package-lock.json",
            "hub-web/package.json",
        ],
        deadline,
    )?;
    if !dependencies.is_empty() {
        return Err(UsageError(
            "Journey dependencies differ; cannot reuse delivered node_modules for the base".into(),
        )
        .into());
    }
    let staging_root = request.cas_root.join("tmp");
    fs::create_dir_all(&staging_root)?;
    let staging = tempfile::Builder::new()
        .prefix("capture-")
        .tempdir_in(&staging_root)?;
    let hooks = staging.path().join("hooks");
    fs::create_dir_all(&hooks)?;
    let run_root = dir.join("journey-runs").join(format!(
        "{}-{}",
        chrono::Utc::now().timestamp_millis(),
        std::process::id()
    ));
    fs::create_dir_all(&run_root)?;
    let mut selected: Option<Vec<String>> = None;
    let mut runs = BTreeMap::new();
    for (side, revision) in [("before", base), ("after", head)] {
        let root = staging.path().join(side);
        git(
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
        git(
            &root,
            &[
                "-c",
                &format!("core.hooksPath={}", hooks.display()),
                "checkout",
                "--quiet",
                "--detach",
                revision,
            ],
            deadline,
        )?;
        reuse_dependencies(&request.root, &root, deadline)?;
        for path in INSTRUMENTATION {
            // git() trims text output; preserve committed bytes for the exact
            // overlay check rather than rewriting line endings or newlines.
            let output = bounded(
                Command::new("git")
                    .current_dir(&request.root)
                    .args(["show", &format!("{head}:{path}")]),
                deadline,
            )?;
            let target = root.join(path);
            let parent = target.parent().context("journey instrumentation target has no parent")?;
            fs::create_dir_all(parent).with_context(|| format!(
                "cannot create journey instrumentation directory for {path}"
            ))?;
            fs::write(target, &output.stdout)?;
        }
        let artifacts = run_root.join(side);
        let mut plan = Command::new("python3");
        plan.current_dir(&root)
            .arg(root.join("scripts/journey-receipt.py"))
            .args(["plan", "--repo"])
            .arg(&root)
            .arg(&artifacts)
            .args(["--compare", base, head]);
        let output = bounded(&mut plan, deadline)?;
        if !output.status.success() {
            anyhow::bail!("journey plan failed");
        }
        let plan: Value = serde_json::from_slice(&output.stdout)?;
        let ids: Vec<String> = serde_json::from_value(plan["selection_ids"].clone())?;
        if ids.is_empty() || ids.len() > config.max_shots as usize {
            return Err(UsageError("Affected journeys are empty or exceed max_shots; configure a representative qa.capture.shots catalog".into()).into());
        }
        if selected.as_ref().is_some_and(|previous| *previous != ids) {
            anyhow::bail!("journey selection mismatch");
        }
        selected = Some(ids);
        // Independent leases for both config web servers; held through teardown.
        let (fixtures_port, fixtures_lease) = lease_port(&staging_root)?;
        let (journeys_port, journeys_lease) = lease_port(&staging_root)?;
        let mut command = Command::new("bash");
        command
            .current_dir(&root)
            .arg(root.join("scripts/journey-eval.sh"))
            .arg(&artifacts)
            .args(["--compare", base, head])
            .env("JOURNEY_RECORD", "1")
            .env("JOURNEY_CAPTURE_MASK", serde_json::to_string(&config.mask)?)
            .env(
                "JOURNEY_CAPTURE_MAX_HTML_BYTES",
                (u64::from(config.max_html_mb) * 1024 * 1024).to_string(),
            )
            .env("HUB_E2E_PORT", fixtures_port.to_string())
            .env("HUB_JOURNEY_PORT", journeys_port.to_string());
        let output = bounded(&mut command, deadline);
        drop((fixtures_lease, journeys_lease));
        let output = output?;
        fs::write(
            run_root.join(format!("{side}.log")),
            [&output.stdout[..], &output.stderr[..]].concat(),
        )?;
        // A failed run is still retained; only passing IDs on both sides pair.
        let summary: Value =
            serde_json::from_slice(&fs::read(artifacts.join("journey-receipt.json"))?)?;
        if summary["head_sha"] != revision
            || summary["comparison"]["head_sha"] != head
            || summary["comparison"]["base_sha"] != base
        {
            anyhow::bail!("journey provenance mismatch");
        }
        runs.insert(side, (artifacts, summary));
    }
    for id in selected.unwrap_or_default() {
        let mut cell = Pair {
            id: id.clone(),
            surface: id.clone(),
            viewport: [0, 0],
            scheme: "light".into(),
            masked: config.mask.clone(),
            before: None,
            after: None,
            reason: None,
        };
        let mut framing = None;
        for (side, revision) in [("before", base), ("after", head)] {
            let (artifacts, summary) = &runs[side];
            let passed = summary["suite_exit"] == 0
                && summary["errors"].as_array().is_some_and(Vec::is_empty)
                && summary["results"].as_array().is_some_and(|rows| {
                    rows.iter()
                        .any(|row| row["id"] == id && row["status"] == "PASS")
                });
            if !passed {
                cell.reason = Some("Journey assertions did not pass on both revisions".into());
                continue;
            }
            let goal = import_goal(artifacts, &id, config, dir, side, revision, deadline);
            match goal {
                Ok((capture, viewport, scheme, identity)) => {
                    let current = (viewport, scheme.clone(), identity);
                    if framing.as_ref().is_some_and(|old| *old != current) {
                        cell.reason =
                            Some("Journey final-stage identity or framing changed".into());
                    }
                    framing = Some(current);
                    cell.viewport = viewport;
                    cell.scheme = scheme;
                    if side == "before" {
                        cell.before = Some(capture);
                    } else {
                        cell.after = Some(capture);
                    }
                }
                Err(_) => {
                    cell.reason =
                        Some("Passing journey has no matching masked final-stage capture".into())
                }
            }
        }
        if cell.before.is_some() && cell.after.is_some() && cell.reason.is_none() {
            manifest.pairs.push(cell);
        } else {
            cell.reason
                .get_or_insert_with(|| "Final-stage capture unavailable on one revision".into());
            manifest.unpaired.push(cell);
        }
    }
    Ok(())
}

fn import_goal(
    artifacts: &Path,
    id: &str,
    config: &QaCaptureConfig,
    dir: &Path,
    side: &str,
    revision: &str,
    deadline: Deadline,
) -> anyhow::Result<(SideCapture, [u32; 2], String, Value)> {
    if !pairs::valid_id(id) {
        anyhow::bail!("invalid journey identity");
    }
    let root = artifacts.join("journeys").join(id);
    let result: Value = serde_json::from_slice(&fs::read(root.join("result.json"))?)?;
    if result["id"] != id || result["status"] != "PASS" || result["masked"] != json!(config.mask) {
        anyhow::bail!("journey not a masked pass");
    }
    // The producer's summary covers all supplemental parts; refuse any retained
    // failed part as well, never pick whichever part happened to finish last.
    for part in walkdir::WalkDir::new(root.join("parts"))
        .into_iter()
        .filter_map(Result::ok)
    {
        if part.file_name() == "result.json" {
            let value: Value = serde_json::from_slice(&fs::read(part.path())?)?;
            if value["status"] != "PASS" || value["masked"] != json!(config.mask) {
                anyhow::bail!("journey part failed");
            }
        }
    }
    let stage = result["stages"]
        .as_array()
        .and_then(|stages| stages.last())
        .ok_or_else(|| anyhow::anyhow!("journey has no goal stage"))?;
    let name = stage["screenshot"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("journey has no goal screenshot"))?;
    if !name.starts_with('J')
        || !name.ends_with(".png")
        || Path::new(name).components().count() != 1
    {
        anyhow::bail!("invalid goal screenshot");
    }
    let source = root.join(name).canonicalize()?;
    if !source.starts_with(root.canonicalize()?) {
        anyhow::bail!("goal path escaped receipt");
    }
    let viewport = [
        result["viewport"]["width"].as_u64().unwrap_or(0) as u32,
        result["viewport"]["height"].as_u64().unwrap_or(0) as u32,
    ];
    if !config.viewports.iter().any(|bound| {
        bound.split_once('x').is_some_and(|(w, h)| {
            w.parse::<u32>().ok() == Some(viewport[0])
                && h.parse::<u32>()
                    .ok()
                    .is_some_and(|h| viewport[1] > 0 && viewport[1] <= h)
        })
    }) {
        anyhow::bail!("journey viewport exceeds configured budget");
    }
    let scheme = result["color_scheme"]
        .as_str()
        .filter(|s| matches!(*s, "light" | "dark"))
        .ok_or_else(|| anyhow::anyhow!("journey scheme absent"))?
        .to_string();
    let png = format!("{id}.{side}.png");
    let jpg = format!("{id}.{side}.jpg");
    fs::copy(source, dir.join(&png))?;
    let mut command = Command::new("python3");
    command.args(["-I","-c","from PIL import Image; import sys; Image.open(sys.argv[1]).convert('RGB').save(sys.argv[2],quality=80)"])
        .arg(dir.join(&png)).arg(dir.join(&jpg));
    let output = bounded(&mut command, deadline)?;
    if !output.status.success() {
        anyhow::bail!("JPEG conversion unavailable");
    }
    Ok((
        SideCapture {
            sha256: pairs::digest(&dir.join(&png))?,
            jpg_sha256: pairs::digest(&dir.join(&jpg))?,
            png,
            jpg,
            reference: revision.into(),
            source: format!("journey-{side}"),
            captured_at: chrono::Utc::now().to_rfc3339(),
        },
        viewport,
        scheme,
        result["title_path"].clone(),
    ))
}

fn bounded(command: &mut Command, deadline: Deadline) -> anyhow::Result<std::process::Output> {
    bounded_process::run_command(command, deadline, deadline.remaining())
        .map_err(|_| anyhow::anyhow!("journey capture exceeded its deadline or could not execute"))
}
