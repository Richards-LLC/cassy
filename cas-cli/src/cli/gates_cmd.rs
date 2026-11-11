//! `cas gates`: the factory check registry and its per-code facts (cas-b5cd).

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, bail};
use clap::{Args, Subcommand};

use super::Cli;
use crate::gates::{Family, REGISTRY, UNCODED, facts};

#[derive(Subcommand)]
pub enum GatesCommands {
    /// List every registered check with its stable code and count by family
    List,
    /// Refusals, overrides and false positives per code since a release tag
    Report(ReportArgs),
    /// Record a supervisor-declared false positive of one gate code
    FalsePositive(FalsePositiveArgs),
}

#[derive(Args)]
pub struct ReportArgs {
    /// Git tag (or any commit-ish) whose commit time starts the window
    #[arg(long)]
    pub since: String,
}

#[derive(Args)]
pub struct FalsePositiveArgs {
    /// Registered gate code, for example G-55
    pub code: String,
    /// Why the refusal was wrong
    #[arg(long)]
    pub reason: String,
    /// Task the refusal blocked
    #[arg(long)]
    pub task: Option<String>,
}

pub fn execute(cmd: &GatesCommands, cli: &Cli, cas_root: Option<&Path>) -> anyhow::Result<()> {
    match cmd {
        GatesCommands::List => list(cli.json),
        GatesCommands::Report(args) => {
            let root = cas_root.context("no Cassy store here; run `cas init`")?;
            report(root, &args.since, cli.json)
        }
        GatesCommands::FalsePositive(args) => {
            let root = cas_root.context("no Cassy store here; run `cas init`")?;
            false_positive(root, args)
        }
    }
}

fn family_counts() -> BTreeMap<Family, usize> {
    let mut counts: BTreeMap<Family, usize> = Family::ALL.iter().map(|family| (*family, 0)).collect();
    for row in REGISTRY {
        *counts.entry(row.family()).or_default() += 1;
    }
    counts
}

fn list(json: bool) -> anyhow::Result<()> {
    let counts = family_counts();
    if json {
        let families: BTreeMap<&str, usize> = counts.iter().map(|(family, n)| (&family.label()[..1], *n)).collect();
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({
            "total": REGISTRY.len(), "families": families, "rows": REGISTRY,
        }))?);
        return Ok(());
    }
    for row in REGISTRY {
        println!("{:<6} {:<12} {}  ({})", row.code, row.stage, row.title, row.source);
    }
    println!();
    for (family, count) in &counts {
        println!("{count:>4}  {}", family.label());
    }
    println!("{:>4}  total checks, measured from gate[...] declarations", REGISTRY.len());
    Ok(())
}

fn tag_time(tag: &str) -> anyhow::Result<chrono::DateTime<chrono::Utc>> {
    let output = std::process::Command::new("git")
        .args(["log", "-1", "--format=%cI", tag, "--"])
        .output()
        .context("run git log")?;
    if !output.status.success() {
        bail!("cannot resolve {tag}: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    let stamp = String::from_utf8(output.stdout)?;
    Ok(chrono::DateTime::parse_from_rfc3339(stamp.trim())
        .with_context(|| format!("parse commit time of {tag}"))?
        .with_timezone(&chrono::Utc))
}

fn report(cas_root: &Path, since: &str, json: bool) -> anyhow::Result<()> {
    let start = tag_time(since)?;
    let store = crate::store::open_event_store(cas_root)?;
    let counts = facts::summarize(store.as_ref(), start)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({
            "since": since, "since_time": start.to_rfc3339(), "codes": counts,
        }))?);
        return Ok(());
    }
    println!("Gate facts since {since} ({})", start.to_rfc3339());
    println!("{:<8} {:>8} {:>10} {:>14}  title", "code", "refused", "overridden", "false positive");
    let mut total = facts::Counts::default();
    for (code, n) in &counts {
        let title = crate::gates::find(code).map_or(
            if code == UNCODED { "refusal without a registered code" } else { "(not in this registry)" },
            |row| row.title,
        );
        println!("{code:<8} {:>8} {:>10} {:>14}  {title}", n.refused, n.overridden, n.false_positive);
        total.refused += n.refused;
        total.overridden += n.overridden;
        total.false_positive += n.false_positive;
    }
    println!("{:<8} {:>8} {:>10} {:>14}  across {} codes", "total", total.refused, total.overridden, total.false_positive, counts.len());
    Ok(())
}

fn false_positive(cas_root: &Path, args: &FalsePositiveArgs) -> anyhow::Result<()> {
    let role = std::env::var("CAS_AGENT_ROLE").unwrap_or_default();
    if role.eq_ignore_ascii_case("worker") {
        bail!("only a supervisor or the operator declares a gate false positive; ask the supervisor");
    }
    if args.reason.trim().is_empty() {
        bail!("--reason must say why the refusal was wrong");
    }
    let store = crate::store::open_event_store(cas_root)?;
    facts::record_false_positive(store.as_ref(), &args.code, args.task.as_deref(), &args.reason)?;
    println!("Recorded a false positive of {}.", args.code);
    Ok(())
}
