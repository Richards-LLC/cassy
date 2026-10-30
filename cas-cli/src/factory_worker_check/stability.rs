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
    /// Version 2: touched test id -> its exact compiled names.
    #[serde(default)]
    pub(super) cases: BTreeMap<String, Vec<String>>,
    /// Version 2: compiled name -> its five trial durations in seconds.
    #[serde(default)]
    pub(super) durations: BTreeMap<String, Vec<f64>>,
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
pub(super) struct Plan {
    pub(super) base: String,
    pub(super) tests: Vec<PlannedTest>,
}

#[derive(Deserialize)]
pub(super) struct PlannedTest { pub(super) id: String, pub(super) fingerprint: String }

/// Touched tests at a delivery that lack each kind of commit-bound evidence.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Touched {
    /// No five-run concurrent pass.
    pub(crate) unstable: Vec<String>,
    /// No failing run before the change (fail-first).
    pub(crate) never_red: Vec<String>,
    /// Over the runtime budget, with the measured median.
    pub(crate) over_budget: Vec<(String, String)>,
}

/// Unit (`--lib`) and integration (`--test`) budgets, in seconds.
const UNIT_BUDGET: f64 = 1.0;
const INTEGRATION_BUDGET: f64 = 10.0;

/// Resolve Cargo's target with the runtime's TOML parser. The embedded Python
/// path therefore keeps supporting hosts whose system Python predates 3.11.
pub(super) fn target_metadata(repo: &Path, test: &TargetedTest) -> Result<Option<String>> {
    if super::manifest_roots(repo).is_empty() { return Ok(None); }
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
        "manifest": manifest,
        "entry": manifest.parent().context("package directory")?.join(relative),
    }))?))
}

impl Proof {
    pub(super) fn valid(&self, base: &str, head: &str) -> bool {
        matches!(self.version, 1 | 2) && self.base == base && self.head == head
            && self.runs == 5 && self.parallelism == 5 && self.retries == 0
            && !self.tests.is_empty() && !self.names.is_empty()
            && self.trials.len() == 5
            && self.trials.iter().all(|run| run.exit_code == 0
                && run.count == self.names.len() as u64
                && run.started.is_finite() && run.ended.is_finite() && run.started < run.ended)
            && self.trials.iter().map(|run| run.started).fold(f64::NEG_INFINITY, f64::max)
                < self.trials.iter().map(|run| run.ended).fold(f64::INFINITY, f64::min)
    }

    /// The slowest case's median trial time, or why none can be measured.
    fn median(&self, id: &str) -> Result<f64, String> {
        let names = self.cases.get(id).filter(|names| !names.is_empty())
            .ok_or("no per-case durations; rerun through the current capped runner")?;
        names.iter().map(|name| {
            let mut times = self.durations.get(name).cloned().unwrap_or_default();
            if times.len() != 5 || times.iter().any(|time| !time.is_finite()) {
                return Err(format!("{name} lacks five trial durations"));
            }
            times.sort_by(f64::total_cmp);
            Ok(times[2])
        }).try_fold(0.0_f64, |slowest, median| median.map(|median| slowest.max(median)))
    }
}

/// Planning inspects immutable Git blobs; it never builds or executes tests.
pub(super) fn plan(repo: &Path, target: &str, head: &str) -> Result<Plan> {
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

/// Every touched test at `head` needs a concurrent pass within its budget
/// and a failing run before the change; park only reads these receipts.
pub(super) fn evaluate(
    repo: &Path, target: &str, head: &str,
    records: &[(String, String, TestReceipt)], reds: &[super::red::Red],
) -> Result<Touched> {
    let plan = plan(repo, target, head)?;
    let mut touched = Touched::default();
    for test in &plan.tests {
        let proofs = records.iter().filter_map(|(receipt_head, _, receipt)| {
            receipt.stability.as_ref()
                .filter(|proof| proof.valid(&plan.base, receipt_head) && proof.tests.contains(&test.id))
                .map(|proof| (proof, receipt.harness.is_some()))
        }).collect::<Vec<_>>();
        if proofs.is_empty() {
            touched.unstable.push(test.id.clone());
        } else {
            let measured = proofs.iter().map(|(proof, integration)| {
                let budget = if *integration { INTEGRATION_BUDGET } else { UNIT_BUDGET };
                proof.median(&test.id).map(|median| (median, budget))
            }).collect::<Vec<_>>();
            if !measured.iter().any(|result| matches!(result, Ok((median, budget)) if median <= budget)) {
                let reason = match &measured[0] {
                    Ok((median, budget)) => format!("median {median:.3}s over the {budget}s budget"),
                    Err(reason) => reason.clone(),
                };
                touched.over_budget.push((test.id.clone(), reason));
            }
        }
        if !reds.iter().any(|red| red.proves(repo, head, test)) {
            touched.never_red.push(test.id.clone());
        }
    }
    Ok(touched)
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
