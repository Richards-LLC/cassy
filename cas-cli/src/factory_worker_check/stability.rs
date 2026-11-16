//! Changed-test stability evidence shares the capped runner's receipt store.
//! Park shares the test receipt's input/ancestry validation; adapters run tests.
use super::*;

pub(crate) mod rust;

pub(crate) const REQUIRED_PASSES: u32 = 5;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Identity {
    pub path: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Proof {
    pub runner: String,
    pub tests: Vec<Identity>,
    pub consecutive_passes: u32,
    pub elapsed_ms: u128,
}

pub(super) fn valid(proof: &Proof) -> bool {
    matches!(proof.runner.as_str(), "nextest" | "vitest" | "jest" | "playwright")
        && proof.consecutive_passes >= REQUIRED_PASSES
        && !proof.tests.is_empty()
        && proof.tests.iter().all(|test| !test.name.is_empty()
            && !test.path.is_empty() && !Path::new(&test.path).is_absolute()
            && Path::new(&test.path).components().all(|part|
                matches!(part, std::path::Component::Normal(_))))
}

fn records(cas_root: &Path, repo: &Path, head: &str) -> Result<Vec<(PathBuf, CheckReceipt)>> {
    let repo = repo.canonicalize()?;
    let directory = receipt_path(cas_root, &repo, head).parent().context("receipt directory")?.to_owned();
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut records = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() { continue; }
        let Ok(record) = serde_json::from_slice::<CheckReceipt>(&std::fs::read(entry.path())?) else { continue; };
        if record.repo == repo && (record.head == head || receipt_head_is_ancestor(&repo, &record.head, head)) {
            records.push((entry.path(), record));
        }
    }
    Ok(records)
}

pub(crate) fn missing(passing: &PassingTests<'_>, tests: &[Identity]) -> Vec<Identity> {
    if tests.is_empty() { return Vec::new(); }
    let covered = passing.records().iter()
        .filter_map(|(_, _, test)| test.stability.as_ref())
        .filter(|proof| valid(proof)).flat_map(|proof| &proof.tests).collect::<BTreeSet<_>>();
    tests.iter().filter(|test| !covered.contains(*test)).cloned().collect()
}

pub(crate) fn required(repo: &Path, head: &str, target: &str, paths: &[String],
    ranges: Option<&[(String, String)]>) -> Result<Vec<Identity>> {
    let fallback;
    let ranges = if let Some(ranges) = ranges { ranges } else {
        let base = git(repo, &["merge-base", target, head])
            .or_else(|_| git(repo, &["merge-base", &format!("refs/remotes/origin/{target}"), head]))?;
        fallback = vec![(base, head.to_owned())];
        &fallback
    };
    let live = rust::live(repo, head, paths)?;
    let mut required = BTreeSet::new();
    for (from, to) in ranges {
        required.extend(rust::changed(repo, from, to, paths)?.into_iter()
            .map(|test| test.identity).filter(|identity| live.contains(identity)));
        let live_javascript = super::javascript::changed_tests(repo, from, head, paths)?
            .into_iter().collect::<BTreeSet<_>>();
        required.extend(super::javascript::changed_tests(repo, from, to, paths)?.into_iter()
            .filter(|identity| live_javascript.contains(identity)));
    }
    Ok(required.into_iter().collect())
}

/// The command that proves these changed tests, one per runner (cas-1468).
/// A file whose runner is unknown gets each valid form as its own command.
pub(crate) fn rerun_commands(cas_root: &Path, repo: &Path, tests: &[Identity]) -> Vec<String> {
    let command = |runner: &str, files: &str| format!(
        "`cas factory worker-check --cas-root {} -- {runner} {files}`", cas_root.display());
    let mut known: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut unknown = BTreeSet::new();
    let mut rust = false;
    for test in tests {
        if test.path.ends_with(".rs") {
            rust = true;
        } else if let Some(runner) = super::javascript::runner_hint(repo, &test.path) {
            known.entry(runner).or_default().insert(test.path.as_str());
        } else {
            unknown.insert(test.path.as_str());
        }
    }
    let mut commands: Vec<String> = known.into_iter().map(|(runner, files)|
        command(runner, &files.into_iter().collect::<Vec<_>>().join(" "))).collect();
    commands.extend(unknown.into_iter().map(|file| format!(
        "no runner config, dependency or local executable identifies {file}; run the one it uses: {}",
        ["jest", "vitest", "playwright"].map(|runner| command(runner, file)).join(" or "),
    )));
    if rust {
        commands.push("`cargo nextest run -p <crate> --lib -E 'test(<name>)'`".into());
    }
    commands
}

/// A failed rerun may not leave an earlier overlapping five-pass proof usable.
pub(crate) fn invalidate(cas_root: &Path, repo: &Path, head: &str, tests: &[Identity]) -> Result<()> {
    if tests.is_empty() { return Ok(()); }
    for (path, record) in records(cas_root, repo, head)? {
        if record.test.and_then(|test| test.stability)
            .is_some_and(|proof| proof.tests.iter().any(|test| tests.contains(test)))
        {
            std::fs::remove_file(path)?;
        }
    }
    Ok(())
}

/// Non-Rust adapters use the same envelope without claiming Rust crate proof.
/// The caller holds the worker lane and capped admission through execution.
pub(crate) fn record(cas_root: &Path, repo: &Path, head: &str, mut proof: Proof) -> Result<()> {
    if clean_head(repo)? != head || !valid(&proof) {
        bail!("STABILITY_PROOF_INVALID: clean exact tip and five nonempty passing test identities required");
    }
    proof.tests.sort();
    proof.tests.dedup();
    let key = hex::encode(Sha256::digest(serde_json::to_vec(&proof.tests)?));
    let selection = TargetedTest { package: proof.runner.clone(), harness: None,
        filter: format!("test(stability_{key})") };
    let path = test_receipt_path(cas_root, &repo.canonicalize()?, head, &selection);
    let receipt = CheckReceipt {
        head: head.into(), repo: repo.canonicalize()?, packages: Vec::new(),
        test: Some(TestReceipt { filter: selection.filter, harness: None,
            count: proof.tests.len() as u64, stability: Some(proof), selected_log: None,
            host_path_opt_out: None, supervisor_recorded: None }),
    };
    std::fs::create_dir_all(path.parent().context("receipt directory")?)?;
    let mut file = tempfile::NamedTempFile::new_in(path.parent().context("receipt directory")?)?;
    use std::io::Write;
    file.write_all(&serde_json::to_vec(&receipt)?)?;
    file.persist(path)?;
    Ok(())
}
