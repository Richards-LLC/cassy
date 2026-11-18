//! Park proof = what lane CI will run. `worker-check -- lane-impact` asks
//! `scripts/ci-test-impact.py lane --phase plan` for this delivery's own and
//! ring selection, runs it once under the usual caps, and records one test
//! receipt per changed crate so park reads it like any other focused proof.
use super::*;

pub(crate) const REQUEST: &str = "lane-impact";

pub(crate) fn requested(args: &[String]) -> bool {
    args == [REQUEST]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Proof {
    pub mode: String,
    pub build: Vec<String>,
    pub expression: String,
}

pub(crate) struct Selection {
    pub proof: Proof,
    /// Changed crates; each gets the receipt park looks for.
    pub packages: Vec<String>,
}

impl Selection {
    /// Lane CI's full selection: its own phase and its ring, in every mode
    /// (a whole-workspace plan's ring is the workspace). Nothing selected is an
    /// error rather than a PASS that ran zero tests.
    pub(crate) fn from_plan(plan: &serde_json::Value, packages: Vec<String>) -> Result<Self> {
        let text = |value: &serde_json::Value| {
            value
                .as_str()
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
        };
        let (selection, split) = (&plan["plan"], &plan["split"]);
        let mode = selection["mode"]
            .as_str()
            .context("lane plan has no mode")?
            .to_owned();
        let expression = match (text(&split["own"]), text(&split["ring"])) {
            (Some(own), Some(ring)) => format!("({own}) or ({ring})"),
            (Some(only), None) | (None, Some(only)) => only,
            (None, None) => bail!(
                "lane CI selects no tests for this {mode} plan; prove the change with a named test instead"
            ),
        };
        let build = split["build"]
            .as_array()
            .context("lane plan has no build arguments")?
            .iter()
            .filter_map(|arg| arg.as_str().map(str::to_owned))
            .collect();
        Ok(Self {
            proof: Proof {
                mode,
                build,
                expression,
            },
            packages,
        })
    }

    pub(crate) fn summary(&self) -> String {
        format!(
            "Lane impact: {} plan; build {}; selection {} bytes; changed crates {}",
            self.proof.mode,
            self.proof.build.join(" "),
            self.proof.expression.len(),
            self.packages.join(",")
        )
    }

    pub(crate) fn args(&self) -> Vec<String> {
        ["nextest", "run"]
            .map(String::from)
            .into_iter()
            .chain(self.proof.build.iter().cloned())
            .chain([
                "-E".into(),
                self.proof.expression.clone(),
                "--no-fail-fast".into(),
            ])
            .collect()
    }

    /// A stable named identity for the selection, so receipts reuse the focused-proof store.
    fn filter(&self) -> String {
        let key = hex::encode(Sha256::digest(
            serde_json::to_vec(&self.proof).expect("serialize lane proof"),
        ));
        format!("test(lane_impact_{})", &key[..16])
    }

    fn identity(&self, package: &str) -> TargetedTest {
        TargetedTest {
            package: package.into(),
            harness: None,
            filter: self.filter(),
        }
    }

    /// Stands in for the requested test where the runner needs one identity.
    pub(crate) fn targeted(&self) -> TargetedTest {
        self.identity(self.packages.first().map_or("cas", String::as_str))
    }

    pub(crate) fn receipt_paths(&self, cas_root: &Path, repo: &Path, head: &str) -> Vec<PathBuf> {
        self.packages
            .iter()
            .map(|package| test_receipt_path(cas_root, repo, head, &self.identity(package)))
            .collect()
    }
}

/// The selection for the diff from `base` to HEAD, computed by lane CI's script.
pub(crate) fn select(repo: &Path, base: &str) -> Result<Selection> {
    if !repo.join("scripts/ci-test-impact.py").is_file() {
        bail!(
            "lane-impact needs scripts/ci-test-impact.py; prove the change with a named test instead"
        );
    }
    let scratch = tempfile::tempdir()?;
    let out = scratch.path().join("lane.json");
    // CI passes the live merge-base as BASE_SHA; a nonexistent history file
    // keeps the plan to the diff alone, since a worker has no CI history.
    let output = Command::new("python3")
        .current_dir(repo)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .args([
            "scripts/ci-test-impact.py",
            "lane",
            "--phase",
            "plan",
            "--base-sha",
            base,
            "--zero-base-ref",
            base,
            "--history",
        ])
        .arg(scratch.path().join("no-history.json"))
        .arg("--out")
        .arg(&out)
        .output()
        .context("start lane CI's impact selection")?;
    if !output.status.success() {
        bail!(
            "lane CI's impact selection failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let plan: serde_json::Value = serde_json::from_slice(&std::fs::read(&out)?)?;
    let mut packages = plan["plan"]["paths"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|path| path.as_str())
        .filter(|path| path.ends_with(".rs"))
        .filter_map(|path| match widened::scope(repo, path) {
            widened::Scope::Crate(name) => Some(name),
            _ => None,
        })
        .collect::<Vec<_>>();
    packages.sort();
    packages.dedup();
    Selection::from_plan(&plan, packages)
}

/// One passing receipt per changed crate, sharing the run's log.
pub(crate) fn record(
    cas_root: &Path,
    repo: &Path,
    head: &str,
    selection: &Selection,
    count: u64,
    log: Option<PathBuf>,
) -> Result<()> {
    for (package, path) in selection
        .packages
        .iter()
        .zip(selection.receipt_paths(cas_root, repo, head))
    {
        let receipt = CheckReceipt {
            head: head.into(),
            repo: repo.into(),
            packages: vec![package.clone()],
            test: Some(TestReceipt {
                filter: selection.filter(),
                harness: None,
                count,
                stability: None,
                widened: None,
                lane: Some(selection.proof.clone()),
                selected_log: log.clone(),
                host_path_opt_out: None,
                supervisor_recorded: None,
            }),
        };
        std::fs::create_dir_all(path.parent().context("receipt directory")?)?;
        std::fs::write(path, serde_json::to_vec(&receipt)?)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The selection lane CI printed for the QA-bundle fix (cas-72c6), whose ring
    /// failed `mcp_tools_test` after the worker had proved only qa_evidence.
    fn qa_bundle_plan() -> serde_json::Value {
        serde_json::json!({
            "plan": {"mode": "scoped"},
            "split": {
                "build": ["-p", "cas", "--lib", "--test", "mcp_tools_test"],
                "own": "(package(cas) and ((binary(cas) and test(/^qa_evidence_tests::/))))",
                "ring": "((package(cas) and (binary(cas) or binary(mcp_tools_test))) or (package(cas) and binary(cas) and test(gates::registry_tests::))) and not ((package(cas) and ((binary(cas) and test(/^qa_evidence_tests::/)))))"
            }
        })
    }

    #[test]
    fn park_proof_is_the_lane_selection_and_covers_the_changed_crate() {
        let selection = Selection::from_plan(&qa_bundle_plan(), vec!["cas".into()]).unwrap();
        assert!(
            selection
                .proof
                .expression
                .contains("binary(mcp_tools_test)"),
            "ring tests the worker never ran"
        );
        assert!(
            selection.proof.expression.contains("qa_evidence_tests"),
            "own tests"
        );
        assert_eq!(selection.args()[..2], ["nextest", "run"]);

        let (root, repo) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let git = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(repo.path())
                    .status()
                    .unwrap()
                    .success()
            )
        };
        git(&["init", "-q"]);
        git(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "base",
        ]);
        let repo_path = repo.path().canonicalize().unwrap();
        let head = clean_head(&repo_path).unwrap();
        record(root.path(), &repo_path, &head, &selection, 3, None).unwrap();
        assert_eq!(
            passing_test_packages(root.path(), &repo_path, &head),
            ["cas"]
        );
    }

    #[test]
    fn a_workspace_plan_runs_its_whole_ring_and_an_empty_plan_is_not_a_pass() {
        let workspace = serde_json::json!({"plan": {"mode": "workspace"}, "split": {"build": ["--workspace"], "own": null, "ring": "all()"}});
        let selection = Selection::from_plan(&workspace, vec!["cas".into()]).unwrap();
        assert_eq!(
            (
                selection.proof.expression.as_str(),
                selection.proof.build.as_slice()
            ),
            ("all()", ["--workspace".to_string()].as_slice())
        );
        let empty = serde_json::json!({"plan": {"mode": "scoped"}, "split": {"build": ["-p", "cas"], "own": null, "ring": null}});
        assert!(Selection::from_plan(&empty, vec!["cas".into()]).is_err());
    }
}
