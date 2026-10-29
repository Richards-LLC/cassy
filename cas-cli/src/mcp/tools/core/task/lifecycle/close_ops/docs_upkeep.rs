//! Delivery-scoped documentation upkeep (G5/G6). Inputs are immutable snapshots.
use super::*;
use crate::config::{RegistryEnforcement, UpkeepStage};
use crate::hooks::handlers::handlers_events::project_overview;
use registry_upkeep::{Finding, delivery_snapshot, finish, read, tracked, warning};
use std::path::Path;

#[allow(clippy::too_many_arguments)]
pub(super) fn at_park(
    notes: &mut CloseNoteTracker<'_>,
    task: &mut Task,
    cas_root: &Path,
    repo: &Path,
    target: &str,
    window: Option<&TaskCommitReceiptWindow>,
    receipt: Option<&str>,
    override_reason: Option<&str>,
) -> Result<Vec<String>, String> {
    let config = match crate::config::Config::load(cas_root) {
        Ok(config) => config,
        Err(error) => {
            return warning(
                notes,
                task,
                format!("UPKEEP WARNING: docs config unreadable (not blocking): {error}"),
            );
        }
    };
    let upkeep = config.upkeep();
    if upkeep.codemap() == UpkeepStage::Off && upkeep.product_overview() == UpkeepStage::Off {
        return Ok(vec![]);
    }
    let (base, head, paths) = match delivery_snapshot(task, repo, target, window, receipt) {
        Ok(snapshot) => snapshot,
        Err(message) => return warning(notes, task, message),
    };
    let qa = config.qa();
    let result = (|| -> anyhow::Result<_> {
        let mut changes = crate::qa_impact::io::changes_for_diff(repo, &base, Some(&head))?;
        if let Some(paths) = paths {
            changes.retain(|c| {
                paths.contains(&c.file) || c.old_file.as_ref().is_some_and(|p| paths.contains(p))
            });
        }
        let before_paths = tracked(repo, &base)?;
        let after_paths = tracked(repo, &head)?;
        let codemap = read(repo, &head, ".claude/CODEMAP.md", &after_paths)?;
        let overview_text = read(repo, &head, "docs/PRODUCT_OVERVIEW.md", &after_paths)?;
        let overview = overview_text.is_some();
        // A tracked .cas/config.toml is the project's config for this delivery;
        // when .cas/ is ignored, the local project config is the override.
        let config_section = match read(repo, &head, ".cas/config.toml", &after_paths)? {
            Some(text) => project_overview::config_section(&text),
            None => config.project_overview.clone(),
        };
        let watch = project_overview::resolve_watch_targets(
            config_section,
            overview_text.as_deref(),
            || after_paths.clone(),
        );
        let mut warnings = Vec::new();
        let g6_on = upkeep.product_overview() != UpkeepStage::Off;
        if let Some(warning) = watch.warning.as_ref().filter(|_| overview && g6_on) {
            warnings.push(format!("UPKEEP WARNING G6: {warning}"));
        }
        let mut registry = |revision: &str, files: &[String]| -> anyhow::Result<_> {
            match registry_upkeep::read_registry(repo, revision, &qa.registry.path, files) {
                Ok(registry) => Ok(registry),
                Err(error) => {
                    warnings.push(format!("UPKEEP WARNING: {} at {revision} malformed; registry overview signal unavailable (not blocking): {error}", qa.registry.path));
                    Ok(None)
                }
            }
        };
        let before = registry(&base, &before_paths)?;
        let after = registry(&head, &after_paths)?;
        // An unreadable old registry cannot prove a newly added live journey.
        let registry_comparable = before.is_some() || !before_paths.contains(&qa.registry.path);
        let facts = crate::qa_impact::io::docs_upkeep(
            &changes,
            &qa.registry.path,
            codemap.as_deref(),
            overview,
            after.as_ref().filter(|_| registry_comparable),
            before.as_ref(),
            &watch,
            &before_paths,
            &after_paths,
        );
        if codemap.is_some() && !facts.codemap.required {
            let files: Vec<_> = changes
                .iter()
                .filter(|c| c.status.starts_with('A') && c.file != ".claude/CODEMAP.md")
                .filter(|c| {
                    c.file.rsplit_once('/').is_some_and(|(dir, _)| {
                        before_paths
                            .iter()
                            .any(|p| p.starts_with(&format!("{dir}/")))
                    })
                })
                .map(|c| c.file.as_str())
                .collect();
            if !files.is_empty() {
                warnings.push(format!(
                    "UPKEEP ADVISORY G5: added files in existing directories (not blocking): {}",
                    files.join(", ")
                ));
            }
        }
        Ok((facts, warnings))
    })();
    let (facts, warnings) = match result {
        Ok(result) => result,
        Err(error) => {
            return warning(
                notes,
                task,
                format!("UPKEEP WARNING: docs inputs unavailable (not blocking): {error:#}"),
            );
        }
    };
    let mut notices = Vec::new();
    let mut errors = Vec::new();
    for (stage, gate, token, title, path, reasons) in [
        (
            upkeep.codemap(),
            "G5",
            "codemap unchanged:",
            "CODEMAP DRIFT",
            ".claude/CODEMAP.md",
            facts.codemap.reasons,
        ),
        (
            upkeep.product_overview(),
            "G6",
            "overview unchanged:",
            "PRODUCT_OVERVIEW DRIFT",
            "docs/PRODUCT_OVERVIEW.md",
            facts.product_overview.reasons,
        ),
    ] {
        if stage == UpkeepStage::Off {
            continue;
        }
        let stage = match stage {
            UpkeepStage::Observe => RegistryEnforcement::Observe,
            _ => RegistryEnforcement::Enforce,
        };
        let findings = if reasons.is_empty() {
            vec![]
        } else {
            vec![Finding {
                gate,
                token,
                title,
                detail: format!(
                    "This delivery changed paths or journeys that {path} tracks:\n{}",
                    reasons
                        .iter()
                        .map(|r| format!("- {r}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                ),
            }]
        };
        match finish(
            notes,
            task,
            stage,
            path,
            findings,
            &warnings,
            override_reason,
        ) {
            Ok(current) => notices.extend(current),
            Err(error) => errors.push(error),
        }
    }
    notices.sort();
    notices.dedup();
    if errors.is_empty() {
        Ok(notices)
    } else {
        Err(errors.join("\n\n"))
    }
}

#[cfg(test)]
#[path = "docs_upkeep_tests.rs"]
mod tests;
