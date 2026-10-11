//! Repository-local integration identity shared with release-integrate.py.
//! Persist the legacy branch on first use; directory names are never identity.
use std::{path::Path, process::Command};

pub(crate) fn resolve(root: &Path) -> Result<String, String> {
    let git = |args: &[&str]| -> Result<String, String> {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .map_err(|error| error.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    let branch = match git(&["config", "--local", "--get", "cas.integrationBranch"]) {
        Ok(branch) => branch,
        Err(_) => {
            let branches = git(&[
                "for-each-ref",
                "--format=%(refname:short)",
                "refs/heads/integration/",
            ])?;
            let branches = branches.lines().collect::<Vec<_>>();
            let branch = match branches.as_slice() {
                [] => "integration/project".to_owned(),
                [branch] => (*branch).to_owned(),
                // cas-52de: the sweep receipt's tip names the branch the
                // daemon integrates; adopt the one branch whose head it is.
                _ => receipt_branch(&git, &branches).ok_or_else(|| {
                    "Multiple legacy integration branches; set git config --local cas.integrationBranch <branch> after reviewing the sweep receipt".to_owned()
                })?,
            };
            git(&["config", "--local", "cas.integrationBranch", &branch])?;
            branch
        }
    };
    if !branch.starts_with("integration/") {
        return Err("cas.integrationBranch must name an integration/ branch".into());
    }
    git(&["check-ref-format", &format!("refs/heads/{branch}")])?;
    Ok(branch)
}

/// The one integration branch whose head is the merge-sweep receipt's tip.
fn receipt_branch(
    git: &impl Fn(&[&str]) -> Result<String, String>,
    branches: &[&str],
) -> Option<String> {
    let common = git(&["rev-parse", "--path-format=absolute", "--git-common-dir"]).ok()?;
    let receipt = Path::new(&common)
        .parent()?
        .join(".cas/merge-sweeps/integration.json");
    let receipt: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(receipt).ok()?).ok()?;
    let tip = receipt.get("tip")?.as_str().filter(|tip| tip.len() == 40)?;
    let mut heads = branches.iter().filter(|branch| {
        git(&[
            "rev-parse",
            "--verify",
            &format!("refs/heads/{branch}^{{commit}}"),
        ])
        .as_deref()
            == Ok(tip)
    });
    match (heads.next(), heads.next()) {
        (Some(branch), None) => Some((*branch).to_owned()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn git(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }
    #[test]
    fn renamed_checkout_keeps_legacy_integration_identity() {
        let temp = tempfile::tempdir().unwrap();
        let original = temp.path().join("original");
        std::fs::create_dir(&original).unwrap();
        git(&original, &["init", "-b", "main"]);
        git(
            &original,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "--allow-empty",
                "-m",
                "base",
            ],
        );
        git(&original, &["branch", "integration/legacy"]);
        let renamed = temp.path().join("renamed");
        std::fs::rename(&original, &renamed).unwrap();
        assert_eq!(resolve(&renamed).unwrap(), "integration/legacy");
        git(&renamed, &["branch", "integration/another"]);
        assert_eq!(resolve(&renamed).unwrap(), "integration/legacy");
    }
    #[test]
    fn several_legacy_branches_adopt_the_one_at_the_sweep_receipt_tip_cas_52de() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        git(root, &["init", "-b", "main"]);
        let commit = |message: &str| {
            git(
                root,
                &[
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                    "-c",
                    "commit.gpgsign=false",
                    "-c",
                    "core.hooksPath=/dev/null",
                    "commit",
                    "--allow-empty",
                    "-m",
                    message,
                ],
            );
            git(root, &["rev-parse", "HEAD"])
        };
        commit("base");
        git(root, &["branch", "integration/old-release"]);
        let tip = commit("integrated");
        git(root, &["branch", "integration/project"]);
        assert!(resolve(root).unwrap_err().contains("Multiple legacy"));
        let receipt = root.join(".cas/merge-sweeps/integration.json");
        std::fs::create_dir_all(receipt.parent().unwrap()).unwrap();
        std::fs::write(&receipt, format!(r#"{{"tip":"{tip}"}}"#)).unwrap();
        assert_eq!(resolve(root).unwrap(), "integration/project");
        assert_eq!(
            git(root, &["config", "--local", "cas.integrationBranch"]),
            "integration/project"
        );
    }

    #[test]
    fn new_repositories_use_directory_independent_default() {
        let temp = tempfile::tempdir().unwrap();
        git(temp.path(), &["init"]);
        assert_eq!(resolve(temp.path()).unwrap(), "integration/project");
        git(
            temp.path(),
            &["config", "--local", "cas.integrationBranch", "main"],
        );
        assert!(resolve(temp.path()).unwrap_err().contains("integration/"));
    }
}
