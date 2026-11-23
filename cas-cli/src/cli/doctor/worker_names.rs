//! `cas doctor` row for worker branch namespaces (cas-dc08, design v3 row 26).
//! Report only: a registered worker name that shares another worker's or a
//! factory branch's `factory/` namespace (N1), with the way back.

use super::{Check, CheckStatus};
use std::path::Path;

const NAME: &str = "worker names";

pub(super) fn check(cas_root: &Path) -> Check {
    let repo = cas_root.parent().unwrap_or(cas_root);
    match crate::factory_isolation::WorkerNameRegistry::load(cas_root, repo) {
        Ok(names) => check_for(&names.audit()),
        Err(error) => Check::new(NAME, CheckStatus::Warning, format!("cannot audit worker names: {error}")),
    }
}

fn check_for(findings: &[String]) -> Check {
    if findings.is_empty() {
        return Check::new(NAME, CheckStatus::Ok, "no worker name shares another's factory/ namespace");
    }
    Check::new(
        NAME,
        CheckStatus::Warning,
        format!(
            "{} worker name(s) share a factory/ branch namespace:\n{}\nWay back: shut the worker down and respawn it without worker_names so it gets a generated name; doctor changes nothing.",
            findings.len(),
            findings.iter().map(|finding| format!("  {finding}")).collect::<Vec<_>>().join("\n"),
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A registered `ab-cd` beside a worker or branch `ab` is reported with
    /// the way back; a store from before N1 can still hold such a pair.
    #[test]
    fn doctor_reports_a_worker_name_inside_another_namespace_cas_dc08() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let project = tempfile::tempdir().unwrap();
        let repo = project.path();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git").args(args).current_dir(repo).output().unwrap();
            assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
        };
        git(&["init", "-b", "main"]);
        git(&["-c", "user.name=f", "-c", "user.email=f@example.test", "commit", "--allow-empty", "-m", "base"]);
        let root = crate::store::init_cas_dir(repo).unwrap();
        let agents = crate::store::open_agent_store(&root).unwrap();
        let worker = |name: &str| cas_types::Agent::new_with_role(format!("{name}-id"), name.into(), cas_types::AgentRole::Worker);
        agents.register(&worker("calm-lark-16")).unwrap();
        assert!(matches!(check(&root).status, CheckStatus::Ok), "{}", check(&root).message);

        git(&["update-ref", "refs/remotes/origin/factory/calm-lark", "HEAD"]);
        let report = check(&root);
        assert!(matches!(report.status, CheckStatus::Warning), "{}", report.message);
        assert!(report.message.contains("WORKER_NAME_OVERLAPS"), "{}", report.message);
        assert!(report.message.contains("factory/calm-lark"), "{}", report.message);
    }
}
