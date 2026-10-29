//! Commit-bound evidence for five concurrent executions of touched Rust tests.
use super::*;

pub(super) const SCRIPT: &str = concat!(
    include_str!("../../../scripts/rust_test_source.py"),
    "\n",
    include_str!("../../../scripts/worker-test-stability.py"),
);

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Proof {
    pub(super) version: u32,
    pub(super) base: String,
    pub(super) head: String,
    pub(super) tests: Vec<String>,
    pub(super) names: Vec<String>,
    pub(super) runs: u32,
    pub(super) parallelism: u32,
    pub(super) retries: u32,
    pub(super) trials: Vec<Trial>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Trial {
    exit_code: i32,
    count: u64,
    started: f64,
    ended: f64,
    log: String,
}

#[derive(Deserialize)]
struct Plan {
    base: String,
    tests: Vec<PlannedTest>,
}

#[derive(Deserialize)]
struct PlannedTest { id: String }

/// Resolve Cargo's target with the runtime's TOML parser. The embedded Python
/// path therefore keeps supporting hosts whose system Python predates 3.11.
pub(super) fn target_metadata(repo: &Path, test: &TargetedTest) -> Result<Option<String>> {
    if !repo.join("Cargo.toml").is_file() { return Ok(None); }
    let (manifest, value) = super::package_manifest(repo, &test.package)?;
    let relative = match &test.harness {
        None => value.get("lib").and_then(|v| v.get("path"))
            .and_then(toml::Value::as_str).unwrap_or("src/lib.rs").to_owned(),
        Some(harness) => value.get("test").and_then(toml::Value::as_array)
            .and_then(|targets| targets.iter().find(|v| v.get("name").and_then(toml::Value::as_str) == Some(harness)))
            .and_then(|v| v.get("path")).and_then(toml::Value::as_str)
            .map(str::to_owned).unwrap_or_else(|| format!("tests/{harness}.rs")),
    };
    Ok(Some(serde_json::to_string(&serde_json::json!({
        "package": test.package, "harness": test.harness,
        "entry": manifest.parent().context("package directory")?.join(relative),
    }))?))
}

impl Proof {
    pub(super) fn valid(&self, base: &str, head: &str) -> bool {
        self.version == 1 && self.base == base && self.head == head
            && self.runs == 5 && self.parallelism == 5 && self.retries == 0
            && !self.tests.is_empty() && !self.names.is_empty()
            && self.trials.len() == 5
            && self.trials.iter().all(|run| run.exit_code == 0
                && run.count == self.names.len() as u64
                && run.started.is_finite() && run.ended.is_finite() && run.started < run.ended)
            && self.trials.iter().map(|run| run.started).fold(f64::NEG_INFINITY, f64::max)
                < self.trials.iter().map(|run| run.ended).fold(f64::INFINITY, f64::min)
    }
}

/// Planning inspects immutable Git blobs; it never builds or executes tests.
fn plan(repo: &Path, target: &str, head: &str) -> Result<Plan> {
    let remote = if target.starts_with("origin/") { target.to_owned() } else { format!("origin/{target}") };
    let base = super::git(repo, &["merge-base", &remote, head])
        .or_else(|_| super::git(repo, &["merge-base", target, head]))?;
    let output = Command::new("python3")
        .args(["-I", "-B", "-c", SCRIPT, "--plan", "--base", &base, "--head", head])
        .current_dir(repo)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output().context("plan touched Rust tests")?;
    if !output.status.success() {
        bail!("cannot resolve touched Rust tests: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    serde_json::from_slice(&output.stdout).context("invalid touched Rust test plan")
}

pub(super) fn missing(
    repo: &Path, target: &str, head: &str,
    records: &[(String, String, TestReceipt)],
) -> Result<Vec<String>> {
    let plan = plan(repo, target, head)?;
    let covered = records.iter().filter_map(|(receipt_head, _, test)| {
        test.stability.as_ref().filter(|proof| proof.valid(&plan.base, receipt_head))
    }).flat_map(|proof| proof.tests.iter()).collect::<BTreeSet<_>>();
    Ok(plan.tests.into_iter().filter_map(|test| (!covered.contains(&test.id)).then_some(test.id)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stability_receipt_requires_bound_complete_concurrent_zero_retry_trials() {
        let original = serde_json::json!({
            "version": 1, "base": "base", "head": "head", "tests": ["src/lib.rs::test_case"],
            "names": ["test_case"], "runs": 5, "parallelism": 5, "retries": 0,
            "trials": (0..5).map(|_| serde_json::json!({
                "exit_code": 0, "count": 1, "started": 0.0, "ended": 1.0, "log": "fixture"
            })).collect::<Vec<_>>()
        });
        let proof: Proof = serde_json::from_value(original.clone()).unwrap();
        assert!(proof.valid("base", "head"));
        for field in ["base", "head", "runs", "parallelism", "retries", "count", "exit_code", "sequential", "missing trial"] {
            let mut value = original.clone();
            match field {
                "base" | "head" => value[field] = "different".into(),
                "runs" | "parallelism" => value[field] = 4.into(),
                "retries" => value[field] = 1.into(),
                "count" => value["trials"][0][field] = 0.into(),
                "exit_code" => value["trials"][0][field] = 1.into(),
                "missing trial" => { value["trials"].as_array_mut().unwrap().pop(); }
                "sequential" => for (i, trial) in value["trials"].as_array_mut().unwrap().iter_mut().enumerate() {
                    trial["started"] = (i as f64).into();
                    trial["ended"] = (i as f64 + 0.5).into();
                },
                _ => unreachable!(),
            }
            let proof: Proof = serde_json::from_value(value).unwrap();
            assert!(!proof.valid("base", "head"), "accepted invalid {field} evidence");
        }
    }
}
