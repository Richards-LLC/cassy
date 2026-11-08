//! G7 report quality (cas-5d07): a report a delivery changes reaches the
//! operator only when it is the report cas-html-reports describes. Visual QA
//! proves the page renders; this gate proves the brief, the critique floor,
//! the brand tokens, a verdict first, the height budget and audience-clean
//! text, and lists exactly what is missing. Inputs are the delivery snapshot.
use super::*;
use crate::config::RegistryEnforcement;
use crate::deliverable::report_quality;
use registry_upkeep::{Finding, delivery_snapshot, finish, read, tracked, warning};
use std::path::Path;

#[allow(clippy::too_many_arguments)]
pub(super) fn at_park(
    notes: &mut CloseNoteTracker<'_>,
    task: &mut Task,
    repo: &Path,
    target: &str,
    window: Option<&TaskCommitReceiptWindow>,
    receipt: Option<&str>,
    override_reason: Option<&str>,
) -> Result<Vec<String>, String> {
    let (base, head, paths) = match delivery_snapshot(task, repo, target, window, receipt) {
        Ok(snapshot) => snapshot,
        Err(message) => return warning(notes, task, message.replace("UPKEEP WARNING", "REPORT QUALITY WARNING")),
    };
    let result = (|| -> anyhow::Result<Vec<(String, String)>> {
        let changes = crate::qa_impact::io::changes_for_diff(repo, &base, Some(&head))?;
        let after = tracked(repo, &head)?;
        let mut reports = Vec::new();
        for change in changes.iter().filter(|change| !change.status.starts_with('D')) {
            if paths.as_ref().is_some_and(|paths| !paths.contains(&change.file)) {
                continue;
            }
            let Some((_, brief_path)) = report_quality::report_paths(&change.file, &after) else {
                continue;
            };
            let Some(html) = read(repo, &head, &change.file, &after)? else { continue };
            let brief = read(repo, &head, &brief_path, &after)?;
            let findings = report_quality::check(&html, brief.as_deref());
            if !findings.is_empty() {
                let lines: Vec<String> = findings.iter().map(|finding| format!("- {}", finding.message)).collect();
                reports.push((change.file.clone(), lines.join("\n")));
            }
        }
        Ok(reports)
    })();
    let reports = match result {
        Ok(reports) => reports,
        Err(error) => {
            return warning(notes, task, format!("REPORT QUALITY WARNING: delivery reports unreadable (not blocking): {error:#}"));
        }
    };
    if reports.is_empty() {
        return Ok(vec![]);
    }
    // Waivers belong to the parked delivery anchor, as for upkeep findings.
    let delivery_head = super::super::qa_evidence_gate::delivered_head(task, repo, target, receipt).unwrap_or(head);
    let mut notices = Vec::new();
    let mut errors = Vec::new();
    for (path, detail) in reports {
        let findings = vec![Finding {
            gate: "G7",
            token: "report quality waived:",
            title: "REPORT NOT READY FOR THE OPERATOR",
            detail: format!(
                "{path} is a report (cas-html-reports), and visual QA alone does not make it one. Missing:\n{detail}"
            ),
        }];
        match finish(notes, task, repo, &delivery_head, RegistryEnforcement::Enforce, &path, findings, &[], override_reason) {
            Ok(current) => notices.extend(current),
            Err(error) => errors.push(error),
        }
    }
    if errors.is_empty() { Ok(notices) } else { Err(errors.join("\n\n")) }
}

#[cfg(test)]
#[path = "report_quality_tests.rs"]
mod tests;
