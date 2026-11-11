//! Journey registry upkeep for a diff (formerly park gates G1/G2, G-49). It
//! runs in the lane lint through `cas qa journeys --check --base <ref>`
//! (cas-4b33); park no longer runs it. All content comes from Git snapshots.
use crate::config::{QaConfig, RegistryEnforcement};
use crate::qa_impact::io::git;
use crate::qa_registry::{DuplicateBugFamily, Problem, Registry};
use anyhow::Result;
use std::path::Path;

/// One upkeep finding: the former gate, its refusal heading and the detail.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Finding {
    pub gate: &'static str,
    pub title: &'static str,
    pub detail: String,
}

#[derive(Debug, Default, serde::Serialize)]
pub struct DiffUpkeep {
    /// Effective policy: the stricter of the registry header at base and head.
    pub stage: Option<RegistryEnforcement>,
    pub findings: Vec<Finding>,
    pub warnings: Vec<String>,
}

impl DiffUpkeep {
    /// Whether the findings refuse under the effective policy.
    pub fn refuses(&self) -> bool {
        !self.findings.is_empty()
            && matches!(self.stage, Some(RegistryEnforcement::Enforce | RegistryEnforcement::Strict))
    }
}

fn tracked(repo: &Path, revision: &str) -> Result<Vec<String>> {
    Ok(git(repo, &["ls-tree", "-rz", "--name-only", revision])?
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect())
}

fn read(repo: &Path, revision: &str, path: &str, paths: &[String]) -> Result<Option<String>> {
    if !paths.iter().any(|p| p == path) {
        return Ok(None);
    }
    git(repo, &["show", &format!("{revision}:{path}")]).map(Some)
}

fn read_registry(repo: &Path, revision: &str, path: &str, paths: &[String]) -> Result<Option<Registry>> {
    crate::qa_registry::load_with(path, paths, |path| read(repo, revision, path, paths))
}

fn problem_lines(problems: &[Problem]) -> String {
    problems
        .iter()
        .map(|p| format!("- {}: {} ({})", p.subject, p.message, p.code))
        .collect::<Vec<_>>()
        .join("\n")
}

fn broken(detail: String) -> Finding {
    Finding { gate: "G1", title: "JOURNEY REGISTRY BROKEN BY THIS DELIVERY", detail }
}

/// The stricter registry policy at base and head; a local setting is fallback
/// only, and an unreadable base cannot authorize a downgrade.
fn stage(repo: &Path, qa: &QaConfig, base: &str, head: &str) -> RegistryEnforcement {
    let rank = |stage: &RegistryEnforcement| match stage {
        RegistryEnforcement::Off => 0,
        RegistryEnforcement::Observe => 1,
        RegistryEnforcement::Enforce => 2,
        RegistryEnforcement::Strict => 3,
    };
    [base, head]
        .into_iter()
        .filter_map(|revision| {
            git(repo, &["show", &format!("{revision}:{}", qa.registry.path)])
                .ok()
                .map(|text| (revision, text))
        })
        .filter_map(|(revision, text)| match crate::qa_registry::parse(&text) {
            Ok(registry) => Some(registry.enforce.unwrap_or(qa.registry.enforce)),
            Err(_) if revision == base => Some(if qa.registry.enforce == RegistryEnforcement::Strict {
                RegistryEnforcement::Strict
            } else {
                RegistryEnforcement::Enforce
            }),
            Err(_) => None,
        })
        .max_by_key(rank)
        .unwrap_or(qa.registry.enforce)
}

/// Registry problems this diff caused (G1) and new surface files no journey
/// owns (G2; strict also checks modified files). Pre-existing debt warns.
pub fn for_diff(repo: &Path, qa: &QaConfig, base: &str, head: &str) -> Result<DiffUpkeep> {
    let mut qa = qa.clone();
    qa.registry.enforce = stage(repo, &qa, base, head);
    let mut out = DiffUpkeep { stage: Some(qa.registry.enforce), ..Default::default() };
    if qa.registry.enforce == RegistryEnforcement::Off {
        return Ok(out);
    }
    let changes = crate::qa_impact::io::changes_for_diff(repo, base, Some(head))?;
    let is_input = |path: &str| crate::qa_registry::is_registry_input(&qa.registry.path, path);
    let registry_touched = changes
        .iter()
        .any(|c| is_input(&c.file) || c.old_file.as_deref().is_some_and(is_input));
    let after_paths = tracked(repo, head)?;
    let registry = match read_registry(repo, head, &qa.registry.path, &after_paths) {
        Ok(None) => return Ok(DiffUpkeep::default()),
        Ok(Some(registry)) => registry,
        Err(error) if error.is::<DuplicateBugFamily>() || registry_touched => {
            out.findings.push(broken(format!(
                "{}: cannot read or parse delivered registry: {error:#}",
                qa.registry.path
            )));
            return Ok(out);
        }
        Err(error) => {
            out.warnings.push(format!(
                "UPKEEP WARNING: {} unreadable or malformed (untouched by this diff): {error:#}",
                qa.registry.path
            ));
            return Ok(out);
        }
    };
    let before_paths = tracked(repo, base)?;
    let old_problems = match read_registry(repo, base, &qa.registry.path, &before_paths) {
        Ok(before) => before.map(|r| r.validate(&before_paths).problems).unwrap_or_default(),
        Err(error) => {
            out.warnings.push(format!("UPKEEP NOTICE: base registry unreadable: {error:#}"));
            Vec::new()
        }
    };
    let problems = registry.validate(&after_paths).problems;
    // Restore only paths removed by this diff. A disappearing glob/spec must
    // have matched at base, and restoring the deletion must resolve it.
    let removed = changes.iter().filter_map(|c| {
        if c.status.starts_with('D') {
            Some(c.file.clone())
        } else if c.status.starts_with('R') {
            c.old_file.clone()
        } else {
            None
        }
    });
    let mut restored_paths = after_paths.clone();
    restored_paths.extend(removed);
    let restored_problems = registry.validate(&restored_paths).problems;
    let (caused, debt): (Vec<_>, Vec<_>) = problems.into_iter().partition(|p| {
        !old_problems.contains(p)
            && if matches!(p.code.as_str(), "orphan_glob" | "untracked_spec") {
                !restored_problems.contains(p)
            } else {
                registry_touched
            }
    });
    if !debt.is_empty() {
        out.warnings.push(format!(
            "UPKEEP WARNING: pre-existing or unrelated registry problems (not blocking):\n{}",
            problem_lines(&debt)
        ));
    }
    if !caused.is_empty() {
        out.findings.push(broken(format!(
            "{}: {} problem(s) this diff caused:\n{}",
            qa.registry.path,
            caused.len(),
            problem_lines(&caused)
        )));
    }
    let impact = match crate::qa_impact::analyze(repo, &qa, &changes, Some(base), Some(head)) {
        Ok(impact) => impact,
        Err(error) if error.is::<crate::qa_impact::io::CatalogDrift>() => {
            out.findings.push(broken(format!("{error:#}")));
            return Ok(out);
        }
        Err(error) => {
            out.warnings.push(format!("UPKEEP WARNING: journey impact unavailable: {error:#}"));
            return Ok(out);
        }
    };
    let strict = qa.registry.enforce == RegistryEnforcement::Strict;
    let unowned: Vec<_> = impact
        .unowned
        .iter()
        .filter(|file| {
            file.surface
                && changes.iter().any(|c| {
                    c.file == file.file
                        && (c.status.starts_with(['A', 'R']) || (strict && c.status.starts_with('M')))
                })
        })
        .map(|file| {
            format!(
                "- {} — {}",
                file.file,
                file.codemap.as_ref().map(|c| c.summary.as_str()).unwrap_or("unmapped CODEMAP area")
            )
        })
        .collect();
    if !unowned.is_empty() {
        out.findings.push(Finding {
            gate: "G2",
            title: "UNOWNED USER-FACING FILE",
            detail: format!(
                "{} {}surface file(s) no journey owns:\n{}\nOwn each file with an owner glob, an owner group, or ignore in {}.",
                unowned.len(),
                if strict { "new or modified " } else { "new " },
                unowned.join("\n"),
                qa.registry.path
            ),
        });
    }
    Ok(out)
}

#[cfg(test)]
#[path = "qa_upkeep_tests.rs"]
mod tests;
