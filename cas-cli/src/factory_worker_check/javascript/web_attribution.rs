//! Reuse lane CI's attribution engine for widened Playwright failures.
use super::*;
use sha2::{Digest, Sha256};

// gate[G-61 park as-of]: Focused proof: exact-tip crate receipts, blast-radius scope and web base attribution
pub(super) fn attribute(
    cas_root: &Path,
    repo: &Path,
    head: &str,
    package: &Path,
    executable: &Path,
    config: &[std::ffi::OsString],
    report: &Value,
    environment: &[(&str, &str)],
    high: u64,
) -> Result<(BTreeSet<(Identity, String)>, String)> {
    let base = crate::factory_worker_check::proof_diff_base(
        cas_root, repo, crate::config::Config::load(cas_root)?.factory().epic_base_branch.as_deref(),
    )?;
    let key = hex::encode(Sha256::digest(serde_json::to_vec(&(repo, package, config, report))?));
    let evidence = cas_root.join("worker-check-web-attribution").join(head).join(key);
    std::fs::create_dir_all(&evidence)?;
    let input = evidence.join("head-report.json");
    let settings = evidence.join("runner.json");
    let output = evidence.join("attribution.json");
    let relative = |path: &Path| -> Result<PathBuf> {
        Ok(path.strip_prefix(repo).context("Playwright input outside delivery")?.to_path_buf())
    };
    std::fs::write(&input, serde_json::to_vec(report)?)?;
    std::fs::write(&settings, serde_json::to_vec(&serde_json::json!({
        "package": relative(package)?, "executable": relative(executable)?,
        "config": config.get(1).map(|path| relative(Path::new(path))).transpose()?,
    }))?)?;
    // Embedded so installed Cassy and downstream projects use the same engine
    // as lane CI, without depending on a source checkout at runtime.
    let helper = evidence.join("ci-test-impact.py");
    std::fs::write(&helper, include_str!("../../../../scripts/ci-test-impact.py"))?;
    let mut command = Command::new("python3");
    command.current_dir(repo).envs(environment.iter().copied())
        .arg(&helper).arg("web-attribute").args(["--base-sha", &base])
        .arg("--web-report").arg(&input).arg("--web-run").arg(&settings)
        .arg("--out").arg(&output);
    let success = super::run_scoped_javascript(command, high, &evidence.join("scope"))?;
    let attribution: Value = serde_json::from_slice(&std::fs::read(&output)
        .context("base attribution did not produce evidence")?)?;
    if attribution["base"] != base {
        bail!(crate::gates::tag("G-61", "base attribution returned a different revision"));
    }
    let tasks = crate::store::open_task_store(cas_root)?;
    let branch = crate::factory_worker_check::git(repo, &["branch", "--show-current"])?;
    let source = tasks.list(None)?.into_iter().find(|task| task.branch.as_deref() == Some(&branch));
    let mut inherited = BTreeSet::new();
    let mut repairs = Vec::new();
    for failure in attribution["inherited"].as_array().context("inherited failures missing")? {
        let path = failure["binary"].as_str().context("inherited journey path")?;
        let name = failure["test"].as_str().context("inherited journey identity")?;
        let project = failure["project"].as_str().context("inherited journey project")?;
        let key = hex::encode(Sha256::digest(serde_json::to_vec(&(&base, failure))?));
        let repair_id = format!("cas-{}", &key[..12]);
        if tasks.get(&repair_id).is_err() {
            let mut repair = crate::types::Task::new(repair_id.clone(), format!("Repair inherited journey: {path} :: {name}"));
            repair.task_type = crate::types::TaskType::Bug;
            repair.priority = cas_types::Priority::HIGH;
            repair.risk = vec![cas_types::TaskRisk::None];
            repair.labels = vec!["release-repair".into()];
            if let Some(source) = &source {
                repair.origin_project = source.origin_project.clone();
                repair.deliverables.work_target = source.deliverables.work_target.clone();
            }
            repair.description = format!(
                "Identical Playwright failure at merge base {base} and delivery {head}.\nFailure: {failure}\nEvidence: {}\nAttribution: {attribution}",
                output.display(),
            );
            // Concurrent deliveries can discover the same base failure. A
            // successful existing row is the same repair, never a second one.
            let epic = source.as_ref().map(|source| tasks.get_parent_epic(&source.id))
                .transpose()?.flatten().map(|epic| epic.id);
            if let Err(error) = tasks.create_atomic(&repair, &[], epic.as_deref(), None) {
                tasks.get(&repair_id).map_err(|_| error)?;
            }
        }
        inherited.insert((Identity::exact(path, name), project.to_string()));
        repairs.push(repair_id);
    }
    let note = format!("WEB_BASE_ATTRIBUTION: head={head} base={base} repairs={} evidence={}", repairs.join(","), output.display());
    if !repairs.is_empty() {
        if let Some(source) = &source {
            tasks.append_note(&source.id, &note)?;
        }
        eprintln!("{note}");
    }
    if !success || attribution["status"] != "inherited" || inherited.is_empty() {
        bail!(crate::gates::tag("G-61", format!("delivery failure or unresolved base comparison; evidence={}", output.display())));
    }
    Ok((inherited, note))
}
