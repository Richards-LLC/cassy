//! Doctor-facing aggregator for `cas integrate` (Phase 3, task **cas-3efe**).
//!
//! `cas doctor` calls [`collect_reports`] to get a structured
//! [`super::types::VerifyReport`] per platform, then [`render_for_doctor`]
//! flattens those into doctor's existing `Check` row format. The opt-in
//! SessionStart banner is computed by [`session_start_banner_text`].
//!
//! ## Severity policy
//!
//! - All-ok / not-configured → doctor row with **status=Ok**.
//! - Any [`super::types::IdState::Stale`] → **status=Warning** with a
//!   `cas integrate <platform> refresh` hint. Stale never escalates to
//!   `Error`: a missing remote ID is fixable by the user, not a Cassy bug.
//! - Any [`super::types::IdState::McpUnreachable`] → **status=Warning** with
//!   a "skipped — MCP not configured" message. Critically *not* an error,
//!   so a missing MCP server in CI doesn't fail the whole doctor run.
//!
//! ## Banner policy
//!
//! Default off. Even with `[integrations] session_start_warn = true`, a banner
//! only fires when at least one platform is `Stale` — `McpUnreachable` and
//! `not_configured` are silent at SessionStart so the codemap banner's
//! signal is preserved.

use std::path::Path;

use super::neon::LiveNeonClient;
use super::types::{IdState, VerifyReport};
use super::{github, neon, vercel};

/// Run all three platforms' `verify_report` and collect the structured
/// reports. Always returns one entry per platform — `not_configured = true`
/// when the SKILL.md is absent, so doctor can render a uniform per-platform
/// row regardless of which platforms the user has actually configured.
pub fn collect_reports(repo_root: &Path) -> Vec<VerifyReport> {
    vec![
        vercel_report(repo_root),
        neon_report(repo_root),
        github::verify_report(repo_root),
    ]
}

fn vercel_report(repo_root: &Path) -> VerifyReport {
    // `default_client()` returns a `mcp-proxy` client when built with that
    // feature, or a stub client that always errors otherwise. In the stub
    // case, every recorded ID becomes `McpUnreachable` — exactly the
    // semantics doctor needs to render "skipped — MCP not configured".
    let client = vercel::default_client();
    vercel::verify_report(repo_root, client.as_ref())
}

fn neon_report(repo_root: &Path) -> VerifyReport {
    // The Live neon client is currently a deliberate placeholder (see
    // cas-1ece module doc). Calling it produces McpUnreachable for every
    // recorded branch — exactly the "doctor can render a 'skip' row"
    // semantics we want here.
    let client = LiveNeonClient::new();
    neon::verify_report(repo_root, &client)
}

/// One doctor row per platform, suitable for embedding in `cas doctor`'s
/// existing `Check { name, status, message }` shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorRow {
    /// e.g. `"vercel integration"`.
    pub name: String,
    pub severity: DoctorSeverity,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoctorSeverity {
    Ok,
    Warning,
}

/// Flatten a vec of [`VerifyReport`] into doctor rows.
///
/// When *every* platform's report has `not_configured = true` AND every
/// note describes an absent file (rather than a parse error), this
/// collapses to a single "no integrations configured" Ok row — the
/// green-field new-repo case.
///
/// A report whose note signals a *parse error* on a present-but-broken
/// SKILL.md (i.e. note text containing `"malformed"` or `"could not"`)
/// blocks the collapse, so the user sees a per-platform row pointing at
/// the broken file instead of a misleading "all clean" row.
pub fn render_for_doctor(reports: &[VerifyReport]) -> Vec<DoctorRow> {
    fn note_is_absence_only(notes: &[String]) -> bool {
        notes.iter().all(|n| {
            let l = n.to_lowercase();
            !l.contains("malformed") && !l.contains("could not")
        })
    }
    if reports
        .iter()
        .all(|r| r.not_configured && note_is_absence_only(&r.notes))
    {
        // No platform has an active integration — emit a single Ok row.
        return vec![DoctorRow {
            name: "integrations".to_string(),
            severity: DoctorSeverity::Ok,
            message: "no integrations configured".to_string(),
        }];
    }
    reports.iter().map(render_one).collect()
}

fn render_one(report: &VerifyReport) -> DoctorRow {
    let name = format!("{} integration", report.platform.as_str());
    if report.not_configured {
        let note = report
            .notes
            .first()
            .cloned()
            .unwrap_or_else(|| "not configured".to_string());
        let lower = note.to_lowercase();
        // Parse error / read failure on a present SKILL.md — surface as a
        // Warning so a corrupted file isn't silently ignored.
        let severity = if lower.contains("malformed") || lower.contains("could not") {
            DoctorSeverity::Warning
        } else {
            DoctorSeverity::Ok
        };
        return DoctorRow {
            name,
            severity,
            message: note,
        };
    }

    let stale: Vec<&str> = report
        .items
        .iter()
        .filter(|i| i.state == IdState::Stale)
        .map(|i| i.id.as_str())
        .collect();
    let unreachable: Vec<&str> = report
        .items
        .iter()
        .filter(|i| matches!(i.state, IdState::McpUnreachable(_)))
        .map(|i| i.id.as_str())
        .collect();

    if !stale.is_empty() {
        // Mention any co-occurring unreachable IDs so a partial transport
        // outage isn't hidden by the (more actionable) stale signal.
        let unreachable_suffix = if unreachable.is_empty() {
            String::new()
        } else {
            format!(
                "; also {} unreachable (MCP not configured)",
                unreachable.len()
            )
        };
        return DoctorRow {
            name,
            severity: DoctorSeverity::Warning,
            message: format!(
                "stale: {} — run `cas integrate {} refresh`{unreachable_suffix}",
                stale.join(", "),
                report.platform.as_str()
            ),
        };
    }
    if !unreachable.is_empty() {
        return DoctorRow {
            name,
            severity: DoctorSeverity::Warning,
            message: format!(
                "skipped — MCP not configured ({} ID{})",
                unreachable.len(),
                if unreachable.len() == 1 { "" } else { "s" }
            ),
        };
    }
    let count = report.items.len();
    DoctorRow {
        name,
        severity: DoctorSeverity::Ok,
        message: format!(
            "{count} ID{} ok",
            if count == 1 { "" } else { "s" }
        ),
    }
}

/// Compute the opt-in SessionStart banner text for the integrations
/// section. Returns `None` when the banner shouldn't fire — either because
/// the config flag is off, or because no platform reported a `Stale` ID.
///
/// `McpUnreachable` and `not_configured` are deliberately silent at
/// SessionStart: they aren't actionable enough to displace the codemap
/// freshness banner.
pub fn session_start_banner_text(reports: &[VerifyReport], opt_in: bool) -> Option<String> {
    if !opt_in {
        return None;
    }
    let stale: Vec<String> = reports
        .iter()
        .filter(|r| r.has_stale())
        .map(|r| {
            let ids: Vec<&str> = r
                .items
                .iter()
                .filter(|i| i.state == IdState::Stale)
                .map(|i| i.id.as_str())
                .collect();
            format!("{}: {}", r.platform.as_str(), ids.join(", "))
        })
        .collect();
    if stale.is_empty() {
        return None;
    }
    Some(format!(
        "stale integration IDs: {}. Run `cas integrate <platform> refresh` to update.",
        stale.join("; ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::integrate::types::{IdState, Platform, VerifyItem, VerifyReport};

    fn vercel_ok() -> VerifyReport {
        let mut r = VerifyReport::ok(Platform::Vercel);
        r.items.push(VerifyItem {
            label: "projectId".into(),
            id: "prj_abc".into(),
            state: IdState::Ok,
        });
        r
    }
    fn neon_stale() -> VerifyReport {
        let mut r = VerifyReport::ok(Platform::Neon);
        r.items.push(VerifyItem {
            label: "production branchId".into(),
            id: "br-prod".into(),
            state: IdState::Ok,
        });
        r.items.push(VerifyItem {
            label: "dev branchId".into(),
            id: "br-dev".into(),
            state: IdState::Stale,
        });
        r
    }
    fn github_unreachable() -> VerifyReport {
        let mut r = VerifyReport::ok(Platform::Github);
        r.items.push(VerifyItem {
            label: "OWNER/REPO".into(),
            id: "alice/repo".into(),
            state: IdState::McpUnreachable("network down".into()),
        });
        r
    }
    fn not_configured(p: Platform) -> VerifyReport {
        VerifyReport::not_configured(p, "no SKILL.md present")
    }

    #[test]
    fn render_for_doctor_emits_one_row_per_platform_when_any_configured() {
        let rows = render_for_doctor(&[vercel_ok(), neon_stale(), github_unreachable()]);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].name, "vercel integration");
        assert_eq!(rows[0].severity, DoctorSeverity::Ok);
        assert!(rows[0].message.contains("ok"));

        assert_eq!(rows[1].severity, DoctorSeverity::Warning);
        assert!(rows[1].message.contains("stale"));
        assert!(rows[1].message.contains("br-dev"));
        assert!(rows[1].message.contains("cas integrate neon refresh"));

        assert_eq!(rows[2].severity, DoctorSeverity::Warning);
        assert!(rows[2].message.contains("MCP not configured"));
    }

    #[test]
    fn render_for_doctor_collapses_to_single_ok_row_when_no_integrations_configured() {
        let rows = render_for_doctor(&[
            not_configured(Platform::Vercel),
            not_configured(Platform::Neon),
            not_configured(Platform::Github),
        ]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "integrations");
        assert_eq!(rows[0].severity, DoctorSeverity::Ok);
        assert!(rows[0].message.contains("no integrations configured"));
    }

    #[test]
    fn render_for_doctor_renders_not_configured_alongside_configured_platforms() {
        // Vercel configured, neon and github not — doctor should show all
        // three rows, with the unconfigured ones marked Ok ("not configured").
        let rows = render_for_doctor(&[
            vercel_ok(),
            not_configured(Platform::Neon),
            not_configured(Platform::Github),
        ]);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].severity, DoctorSeverity::Ok);
        // not-configured row carries the report's first note as the message.
        assert!(
            rows[1].message.contains("SKILL.md present"),
            "expected note to surface: {}",
            rows[1].message
        );
    }

    #[test]
    fn session_start_banner_silent_when_opt_in_is_off() {
        let banner = session_start_banner_text(&[neon_stale()], /*opt_in=*/ false);
        assert!(banner.is_none(), "default-off banner should be silent");
    }

    #[test]
    fn session_start_banner_fires_only_for_stale_when_opted_in() {
        // Stale → banner.
        let banner = session_start_banner_text(&[neon_stale()], /*opt_in=*/ true).unwrap();
        assert!(banner.contains("stale"));
        assert!(banner.contains("br-dev"));
        // McpUnreachable alone → silent (preserves codemap banner signal).
        let banner = session_start_banner_text(&[github_unreachable()], true);
        assert!(banner.is_none());
        // not_configured alone → silent.
        let banner = session_start_banner_text(&[not_configured(Platform::Neon)], true);
        assert!(banner.is_none());
    }

    #[test]
    fn session_start_banner_aggregates_multiple_stale_platforms() {
        let mut vercel_stale = VerifyReport::ok(Platform::Vercel);
        vercel_stale.items.push(VerifyItem {
            label: "projectId".into(),
            id: "prj_dead".into(),
            state: IdState::Stale,
        });
        let banner = session_start_banner_text(&[vercel_stale, neon_stale()], true).unwrap();
        assert!(banner.contains("vercel"));
        assert!(banner.contains("neon"));
        assert!(banner.contains("prj_dead"));
        assert!(banner.contains("br-dev"));
    }
}
