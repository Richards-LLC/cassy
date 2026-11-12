//! One read-only policy evaluation, before preparation and again for changed cut inputs.
use super::*;

#[derive(Debug, Args)]
pub struct PlanArgs {
    #[arg(long)] pub commit: Option<String>,
    #[arg(long)] pub version: Option<String>,
    #[arg(long)] pub train: Option<String>,
    #[arg(long)] pub notes: Option<PathBuf>,
    #[arg(long, default_value = "origin/main")] pub base: String,
    /// Bind the leased runner's plan, blockers and first admission time.
    #[arg(long)] pub admit: bool,
}

#[derive(Serialize, Deserialize)]
pub(super) struct CutPlan {
    pub tip: String,
    pub version: String,
    pub notes: PathBuf,
    pub inputs: Option<ReleaseInputs>,
    pub preview: Option<ReleasePreview>,
    pub blockers: BTreeMap<String, String>,
    pub cut_started_at: Option<String>,
}

fn capture<T>(blockers: &mut BTreeMap<String, String>, code: &str, result: Result<T>) -> Option<T> {
    result.map_err(|error| { blockers.insert(code.into(), error.to_string()); }).ok()
}

fn python(repo: &Path, tip: &str, script: &str, args: &[&str], context: &str) -> Result<()> {
    use std::io::Write;
    let mut source = tempfile::NamedTempFile::new()?;
    source.write_all(git(repo, &["show", &format!("{tip}:scripts/{script}")])?.as_bytes())?;
    let output = Command::new("python3").arg("-I").arg(source.path()).args(args)
        .current_dir(repo).env("CAS_RELEASE_CONTEXT_JSON", context).output()?;
    ensure!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr).trim());
    Ok(())
}

pub(super) fn evaluate(args: &PlanArgs, root: &Path, repo: &Path) -> CutPlan {
    let tip = args.commit.clone().unwrap_or_else(|| "HEAD".into());
    let mut blockers = BTreeMap::new();
    let tip = capture(&mut blockers, "IDENTITY", git(repo, &["rev-parse", "--verify", &format!("{tip}^{{commit}}")])).unwrap_or(tip);
    let train = args.train.as_deref().and_then(|id| capture(&mut blockers, "IDENTITY",
        open_task_store(root).and_then(|store| store.get(id)).map_err(Into::into)));
    if let Some(train) = &train {
        capture(&mut blockers, "IDENTITY", (|| {
            ensure!(crate::mcp::tools::core::task::release_train::is_release_train(train), "expected a release train");
            let context = new_context(repo, train)?;
            ensure!(crate::qa_batch::commit(repo, &context.branch).as_deref() == Some(tip.as_str()), "train tip changed; re-plan");
            ensure!(crate::mcp::tools::core::task::repo_context::repo_answers_to(repo, &train.deliverables.work_target.as_ref().context("train target missing")?.repo_selector), "train repository identity differs");
            Ok(())
        })());
    }
    let version = args.version.clone().or_else(|| git(repo, &["show", &format!("{}:cas-cli/Cargo.toml", args.base)]).ok()
        .and_then(|text| text.lines().find_map(|line| line.strip_prefix("version = \"")).map(|value| value.trim_end_matches('"').to_string()))
        .and_then(|value| { let mut parts = value.split('.'); Some(format!("{}.{}.0", parts.next()?, parts.next()?.parse::<u64>().ok()? + 1)) }))
        .unwrap_or_default();
    let previous = capture(&mut blockers, "VERSION_RECEIPT", (|| -> Result<Option<ReleaseContext>> {
        let path = receipt_path(root, &version)?;
        if !path.exists() { return Ok(None) }
        let value: ReleaseContext = serde_json::from_slice(&fs::read(path)?)?;
        ensure!(train.as_ref().is_some_and(|train| train.id == value.train_id), "version belongs to another train");
        Ok(Some(value))
    })()).flatten();
    let notes = args.notes.clone().or_else(|| train.as_ref().and_then(|train|
        crate::mcp::tools::core::workflow::release_auto_cut::notes_draft(root, &train.id).ok()))
        .unwrap_or_else(|| repo.join("release-notes.md"));
    capture(&mut blockers, "NOTES_LINT", python(repo, &tip, "release-train-announce.py", &["--lint", &notes.to_string_lossy()], "{}"));
    let audit = tempfile::NamedTempFile::new();
    let inputs = capture(&mut blockers, "MEMBERSHIP", (|| -> Result<ReleaseInputs> {
        let audit = audit?;
        python(repo, &tip, "release_changelog.py", &["verify-deliveries", &args.base, "--tip", &tip,
            "--notes", &notes.to_string_lossy(), "--report", &audit.path().to_string_lossy()],
            &previous.as_ref().map(serde_json::to_string).transpose()?.unwrap_or_else(|| "{}".into()))?;
        Ok(serde_json::from_slice(&fs::read(audit.path())?)?)
    })());
    let scope = capture(&mut blockers, "IDENTITY", cut_scope(root, repo, &tip, &version, args.train.as_deref(), None));
    let preview = scope.and_then(|mut scope| {
        if let Some(inputs) = &inputs { scope.included_task_ids = inputs.included_task_ids.clone(); scope.no_code_members = inputs.no_code_members.clone(); }
        capture(&mut blockers, "NO_CODE_PROOF", validate_scope_inputs(root, &scope));
        capture(&mut blockers, "MEMBER_STATE", preview_scope(root, repo, scope))
    });
    if let Some(preview) = &preview { blockers.extend(preview.cut_blockers()); }
    capture(&mut blockers, "QUEUE_CONFIGURATION", (|| {
        let slug = crate::github_repo::origin_slug(repo, std::time::Duration::from_secs(3)).map_err(anyhow::Error::msg)?.context("repository identity missing")?;
        let (owner, name) = slug.split_once('/').context("repository identity malformed")?;
        let mut command = Command::new(crate::github_repo::gh_binary());
        command.args(["api", "graphql", "-F", &format!("owner={owner}"), "-F", &format!("name={name}"), "-f", &format!("branch={}", args.base.strip_prefix("origin/").unwrap_or(&args.base)),
            "-f", "query=query($owner:String!,$name:String!,$branch:String!){repository(owner:$owner,name:$name){mergeQueue(branch:$branch){configuration{maximumEntriesToMerge mergingStrategy}}}}", "--jq", ".data.repository.mergeQueue.configuration"]).current_dir(repo);
        let timeout = std::time::Duration::from_secs(3);
        let out = crate::bounded_process::run_command(&mut command, crate::bounded_process::Deadline::after(timeout), timeout).map_err(|error| anyhow::anyhow!("{error:?}"))?;
        let config: serde_json::Value = serde_json::from_slice(&out.stdout)?;
        ensure!(out.status.success() && config["maximumEntriesToMerge"] == 1 && config["mergingStrategy"] == "ALLGREEN", "Set singleton ALLGREEN landing; retain parallel builds");
        Ok(())
    })());
    CutPlan { tip, version, notes, inputs, preview, blockers, cut_started_at: None }
}

pub fn plan(args: &PlanArgs, root: &Path) -> Result<()> {
    let repo = PathBuf::from(git(&std::env::current_dir()?, &["rev-parse", "--show-toplevel"])?);
    let mut value = evaluate(args, root, &repo);
    if args.admit {
        if let Some(train) = &args.train {
            let path = crate::mcp::tools::core::workflow::release_auto_cut::notes_draft(root, train)?.with_file_name("cut-plan.json");
            value.cut_started_at = fs::read(&path).ok().and_then(|bytes| serde_json::from_slice::<CutPlan>(&bytes).ok()).and_then(|old| old.cut_started_at);
            if value.blockers.is_empty() {
                let at = value.cut_started_at.get_or_insert_with(|| chrono::Utc::now().to_rfc3339()).clone();
                if let Some(inputs) = &mut value.inputs { inputs.metadata.insert("cut_started_at".into(), at.into()); }
            }
            crate::mcp::tools::core::workflow::release_freeze::record_cut_blockers(root, train, &value.tip, &value.blockers)?;
            fs::write(path, serde_json::to_vec_pretty(&value)?)?;
        }
    }
    println!("{}", serde_json::to_string(&value)?);
    ensure!(value.blockers.is_empty(), "RELEASE_PLAN_BLOCKED: {}", serde_json::to_string(&value.blockers)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cut_plan_collects_refusals_before_build_cas_80df() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let (temp, root, train, _, args) = super::super::tests::release_fixture();
        let repo = temp.path();
        env.set("CAS_ROOT", &root);
        let gh = repo.join("gh-fixture");
        crate::test_paths::warm_stub(&gh, "#!/bin/sh\nprintf '{\"maximumEntriesToMerge\":1,\"mergingStrategy\":\"ALLGREEN\"}\\n'\n");
        env.set(crate::github_issue_attach::GH_BIN_ENV, &gh);
        fs::create_dir_all(repo.join("scripts")).unwrap();
        fs::create_dir_all(repo.join("changelog.d")).unwrap();
        fs::write(repo.join("scripts/release-train-announce.py"), "raise SystemExit(5)\n").unwrap();
        fs::write(repo.join("scripts/release_changelog.py"), include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../scripts/release_changelog.py"))).unwrap();
        fs::write(repo.join("changelog.d/cas-abcd.md"), "### Fixed — Reports\n\n- **Report** — Recorded findings.\n").unwrap();
        git(repo, &["add", "scripts", "changelog.d"]).unwrap();
        git(repo, &["commit", "-qm", "plan fixture"]).unwrap();
        let tip = git(repo, &["rev-parse", "HEAD"]).unwrap();
        // A different checkout linter must not overrule the pinned train code.
        fs::write(repo.join("scripts/release-train-announce.py"), "raise SystemExit(0)\n").unwrap();
        let tasks = open_task_store(&root).unwrap();
        let mut report = cas_types::Task::new("cas-abcd".into(), "Report".into());
        report.origin_project = Some("release-fixture".into());
        report.execution_note = Some("no-code".into());
        report.status = TaskStatus::AwaitingMerge;
        tasks.add(&report).unwrap();
        let notes = root.join("notes.md");
        fs::write(&notes, "notes\n").unwrap();
        let mut previous = new_context(repo, &train).unwrap();
        previous.train_id = "cas-other".into();
        save(&receipt_path(&root, &args.version).unwrap(), &previous).unwrap();
        let before = serde_json::to_value(tasks.list(None).unwrap()).unwrap();
        let plan = evaluate(&PlanArgs { commit: Some(tip.clone()), version: Some(args.version), train: Some(train.id), notes: Some(notes), base: "origin/main".into(), admit: false }, &root, repo);
        for code in ["NOTES_LINT", "NO_CODE_PROOF", "VERSION_RECEIPT"] {
            assert!(plan.blockers.contains_key(code), "{code}: {:?}", plan.blockers);
        }
        assert_eq!(git(repo, &["rev-parse", "HEAD"]).unwrap(), tip);
        assert_eq!(serde_json::to_value(tasks.list(None).unwrap()).unwrap(), before);
        assert!(!repo.join("hub-web/dist").exists());
    }
}
