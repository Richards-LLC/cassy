//! `cas update --render-fixture <json>`: replay a recorded update through the
//! real human renderer, for terminal review captures (cas-0aa5).
//!
//! The fixture supplies what the two seams of [`super::run_full_update`]
//! would have produced: the release check, the hub verification and every
//! project's phase results with the output each phase printed. Rendering,
//! phase lines, summaries and the exit verdict all run the shipping code.
//! Nothing is read from or written to a store, the binary or the hub.

use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Context;
use serde::Deserialize;

use super::findings::display_names;
use super::live::set_activity;
use super::{
    BinaryUpdateOutcome, ProjectPhase, ProjectRefreshReceipt, RefreshResults, SkippedProject,
    UpdateArgs, binary_notes, finish_project_refresh, run_full_update, update_interactivity,
};
use crate::cli::Cli;
use crate::cli::hub::HubRestartOutcome;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    /// Where the data came from; never rendered.
    #[allow(dead_code)]
    provenance: String,
    binary: BinaryFixture,
    hub: HubFixture,
    #[serde(default)]
    refresh_ms: u64,
    user_level: String,
    #[serde(default)]
    user_output: String,
    projects: Vec<ProjectFixture>,
    #[serde(default)]
    skipped_unregistered: Vec<SkippedFixture>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BinaryFixture {
    current: String,
    latest: String,
    /// The target triple self_update reported; recorded, not rendered.
    #[allow(dead_code)]
    target: String,
    #[serde(default)]
    elapsed_ms: u64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct HubFixture {
    prior_state: String,
    action: String,
    verified: bool,
    service_managed: bool,
    transport_warning: Option<String>,
    transport_error: Option<String>,
    remedy: Option<String>,
    /// What `cas hub` printed to stdout and stderr during the update.
    stdout: String,
    stderr: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectFixture {
    project: PathBuf,
    #[serde(default = "registered")]
    registered: bool,
    migration: String,
    search_index: String,
    skills: String,
    membership: String,
    cloud_sync: String,
    /// What each phase printed, in phase order.
    #[serde(default)]
    output: PhaseOutput,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct PhaseOutput {
    migration: String,
    search_index: String,
    skills: String,
    membership: String,
    cloud_sync: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SkippedFixture {
    project: PathBuf,
    reason: String,
}

fn registered() -> bool {
    true
}

/// The receipt grammar (`ok: …`, `FAILED: …`) back into a phase.
fn phase(summary: &str) -> anyhow::Result<ProjectPhase> {
    let (kind, detail) = summary
        .split_once(": ")
        .with_context(|| format!("phase summary {summary:?} has no `<kind>: ` prefix"))?;
    let detail = detail.to_owned();
    Ok(match kind {
        "ok" => ProjectPhase::Ok(detail),
        "skipped" => ProjectPhase::Skipped(detail),
        "dry-run" => ProjectPhase::Planned(detail),
        "warning" => ProjectPhase::Warning(detail),
        "FAILED" => ProjectPhase::Failed(detail),
        other => anyhow::bail!("unknown phase kind {other:?} in {summary:?}"),
    })
}

pub(super) fn render(path: &Path, args: &UpdateArgs, cli: &Cli) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("could not read update fixture {}", path.display()))?;
    let fixture: Fixture = serde_json::from_str(&text)
        .with_context(|| format!("invalid update fixture {}", path.display()))?;
    let receipts = fixture
        .projects
        .iter()
        .map(receipt)
        .collect::<anyhow::Result<Vec<_>>>()?;
    let user_level = phase(&fixture.user_level)?;
    let skipped_unregistered = fixture
        .skipped_unregistered
        .iter()
        .map(|skip| SkippedProject {
            project: skip.project.clone(),
            reason: skip.reason.clone(),
        })
        .collect();
    let binary = &fixture.binary;
    let hub = &fixture.hub;
    let interactive = !update_interactivity(args, cli, io::stdin().is_terminal()).no_confirm;
    run_full_update(
        cli,
        interactive,
        || {
            // The release check prints nothing of its own (cas-70a8); the
            // phase line carries it.
            pause(Duration::from_millis(binary.elapsed_ms));
            let updated = binary.latest != binary.current;
            Ok(BinaryUpdateOutcome {
                version: binary.latest.clone(),
                updated,
                post_install_refresh: None,
                pending_post_swap: None,
                notes: binary_notes(updated, &binary.latest, &[]),
            })
        },
        |_| anyhow::bail!("a fixture never swaps the binary"),
        |_| {
            if !cli.json {
                print!("{}", hub.stdout);
                eprint!("{}", hub.stderr);
            }
            Ok(HubRestartOutcome {
                transport_error: hub.transport_error.clone(),
                previous_version: Some(binary.current.clone()),
                current_version: Some(binary.latest.clone()),
                service_managed: hub.service_managed,
                prior_state: hub.prior_state.clone(),
                action: hub.action.clone(),
                verified: hub.verified,
                loopback_verified: hub.verified,
                transport_verified: hub.transport_warning.as_ref().map(|_| false),
                transport_warning: hub.transport_warning.clone(),
                remedy: hub.remedy.clone(),
                ..HubRestartOutcome::default()
            })
        },
        |error, hub_restart| {
            let started_at = Instant::now();
            let per_project =
                Duration::from_millis(fixture.refresh_ms) / (receipts.len().max(1) as u32);
            let names = display_names(receipts.iter().map(|receipt| receipt.project.as_path()));
            for (index, name) in names.iter().enumerate() {
                set_activity(format!("{} of {} · {name}", index + 1, names.len()));
                pause(per_project);
            }
            finish_project_refresh(
                args,
                cli,
                RefreshResults {
                    receipts,
                    user_level,
                    user_details: fixture.user_output.clone(),
                    user_retired_toml: Vec::new(),
                    skipped_unregistered,
                },
                error,
                Some(hub_restart),
                None,
                started_at,
            )
        },
    )
}

fn receipt(project: &ProjectFixture) -> anyhow::Result<ProjectRefreshReceipt> {
    let phases = [
        (phase(&project.migration)?, &project.output.migration),
        (phase(&project.search_index)?, &project.output.search_index),
        (phase(&project.skills)?, &project.output.skills),
        (phase(&project.membership)?, &project.output.membership),
        (phase(&project.cloud_sync)?, &project.output.cloud_sync),
    ];
    let details = phases.iter().map(|(_, output)| output.as_str()).collect();
    let phase_details = phases
        .iter()
        .map(|(phase, output)| (phase.is_ok(), (*output).clone()))
        .collect();
    let [migration, search_index, skills, membership, cloud] = phases.map(|(phase, _)| phase);
    Ok(ProjectRefreshReceipt {
        project: project.project.clone(),
        unregistered: !project.registered,
        retired_toml: None,
        migration,
        search_index,
        skills,
        membership,
        cloud,
        details,
        phase_details,
    })
}

/// Recorded phase time, so live progress has something to show.
fn pause(duration: Duration) {
    if !duration.is_zero() {
        std::thread::sleep(duration);
    }
}
