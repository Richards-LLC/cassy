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
                [] => "integration/project",
                [branch] => branch,
                _ => return Err("Multiple legacy integration branches; set git config --local cas.integrationBranch <branch> after reviewing the sweep receipt".into()),
            };
            git(&["config", "--local", "cas.integrationBranch", branch])?;
            branch.to_owned()
        }
    };
    if !branch.starts_with("integration/") {
        return Err("cas.integrationBranch must name an integration/ branch".into());
    }
    git(&["check-ref-format", &format!("refs/heads/{branch}")])?;
    Ok(branch)
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
