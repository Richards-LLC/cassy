//! Project registry visibility; uses the same coverage data as `qa journeys --status`.
use super::{Check, CheckStatus};

pub(super) fn check(root: &std::path::Path, qa: &crate::config::QaConfig) -> Option<Check> {
    // Registry tooling requires Git; non-checkout stores have no source to seed.
    if !root.join(".git").exists() && !root.join(&qa.registry.path).exists() {
        return None;
    }
    let (status, message) = match crate::qa_seed::status(root, qa) {
        Ok(report) if !report.present => (
            CheckStatus::Warning,
            "Missing; run `cas qa journeys --seed`".into(),
        ),
        Ok(report) if !report.valid => (
            CheckStatus::Warning,
            format!(
                "{} problem(s); run `cas qa journeys --check`",
                report.problems.len()
            ),
        ),
        Ok(report) => (
            if report.debt_count == 0 && report.unowned_surfaces.is_empty() {
                CheckStatus::Ok
            } else {
                CheckStatus::Warning
            },
            format!(
                "{} journeys, {} debt, {} unowned surfaces; run `cas qa journeys --status`",
                report.journeys,
                report.debt_count,
                report.unowned_surfaces.len()
            ),
        ),
        Err(error) => (
            CheckStatus::Warning,
            format!("{error}; run `cas qa journeys --status`"),
        ),
    };
    Some(Check {
        name: "journey registry".into(),
        status,
        message,
    })
}
