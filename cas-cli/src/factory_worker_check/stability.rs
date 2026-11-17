//! Changed-test stability evidence shares the capped runner's receipt store.
//! Park shares the test receipt's input/ancestry validation; adapters run tests.
use super::*;

pub(crate) mod rust;

pub(crate) const REQUIRED_PASSES: u32 = 5;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Identity {
    pub path: String,
    pub name: String,
    /// The name is a parameterized title template (`it.each`): `%s`-style
    /// and `$field` placeholders match each runtime row's interpolated name.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub parameterized: bool,
}

impl Identity {
    pub(crate) fn exact(path: impl Into<String>, name: impl Into<String>) -> Self {
        Self { path: path.into(), name: name.into(), parameterized: false }
    }

    /// The runtime-name regex body (unanchored) this identity selects.
    pub(crate) fn name_pattern(&self) -> String {
        if !self.parameterized {
            return regex::escape(&self.name);
        }
        let mut pattern = String::new();
        let mut chars = self.name.chars().peekable();
        while let Some(ch) = chars.next() {
            match (ch, chars.peek().copied()) {
                ('%', Some('%')) => {
                    chars.next();
                    pattern.push('%');
                }
                ('%', Some('p' | 's' | 'd' | 'i' | 'f' | 'j' | 'o' | 'O' | '#' | '$')) => {
                    chars.next();
                    pattern.push_str(".*");
                }
                ('$', Some(next)) if next.is_ascii_alphabetic() || next == '_' => {
                    while chars.peek().is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.')) {
                        chars.next();
                    }
                    pattern.push_str(".*");
                }
                _ => pattern.push_str(&regex::escape(&ch.to_string())),
            }
        }
        pattern
    }

    /// Whether a runtime test (repository-relative path, full name) is one
    /// this identity selects: the exact name, or any row of its template.
    pub(crate) fn selects(&self, runtime: &Identity) -> bool {
        self.path == runtime.path
            && if self.parameterized {
                regex::Regex::new(&format!("^(?:{})$", self.name_pattern()))
                    .is_ok_and(|pattern| pattern.is_match(&runtime.name))
            } else {
                self.name == runtime.name
            }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Proof {
    pub runner: String,
    pub tests: Vec<Identity>,
    pub consecutive_passes: u32,
    pub elapsed_ms: u128,
}

pub(super) fn valid(proof: &Proof) -> bool {
    matches!(proof.runner.as_str(), "nextest" | "vitest" | "jest" | "playwright" | "node")
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

/// Tests this delivery changed. Rust keeps its per-range ownership adapter,
/// intersected with the same surviving net changes the capped runner repeats. JavaScript
/// reads only the delivered head over the net diff from the scoped proof base
/// (`merge-base(target, head)`), so reverted edits, target content merged into
/// the lane and unparseable intermediate commits never count; with delivery
/// ranges, only lines blamed on the task's own commits count (foreign PR work
/// on the same branch stays out).
pub(crate) fn required(repo: &Path, head: &str, target: &str, paths: &[String],
    ranges: Option<&[(String, String)]>) -> Result<Vec<Identity>> {
    let base = git(repo, &["merge-base", target, head])
        .or_else(|_| git(repo, &["merge-base", &format!("refs/remotes/origin/{target}"), head]))?;
    let fallback = [(base.clone(), head.to_owned())];
    let owned = ranges.map(|ranges| delivery_commits(repo, ranges)).transpose()?;
    let ranges = ranges.unwrap_or(&fallback);
    // Historical ranges establish ownership, not whether a test edit survived.
    // After integration, keep the attributed base rather than collapsing proof
    // onto the already-landed tip. Otherwise compare with the current target.
    let net_base = if base == head { ranges.first().map_or(base.as_str(), |(from, _)| from) } else { &base };
    let live = rust::net_changed(repo, net_base, head)?.into_iter()
        .map(|test| test.identity).collect::<BTreeSet<_>>();
    let mut required = BTreeSet::new();
    for (from, to) in ranges {
        required.extend(rust::changed(repo, from, to, paths)?.into_iter()
            .map(|test| test.identity).filter(|identity| live.contains(identity)));
    }
    required.extend(super::javascript::changed_tests(repo, net_base, head, paths, owned.as_ref())?);
    Ok(required.into_iter().collect())
}

fn delivery_commits(repo: &Path, ranges: &[(String, String)]) -> Result<BTreeSet<String>> {
    let mut commits = BTreeSet::new();
    for (from, to) in ranges {
        commits.extend(git(repo, &["rev-list", &format!("{from}..{to}")])?
            .lines().map(str::to_owned));
    }
    Ok(commits)
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
        // cas-e751: outside the worker hook the capped runner needs --cas-root.
        commands.push(format!("`cargo nextest run -p <crate> --lib -E 'test(<name>)'` (the worker hook caps it; elsewhere {})",
            command("nextest run -p <crate> --lib -E", "'test(<name>)'")));
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
