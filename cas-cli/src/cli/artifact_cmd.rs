//! `cas artifact publish|show|list` — the one call a worker makes to turn a
//! local file into a durable, citable artifact (cassy#910).
//!
//! Human output follows `cas-cli-craft`: a verdict line first, then the facts
//! that justify it, then one `->` hint. `--json` is a single document and is
//! printed instead of the human form, never alongside it.

use anyhow::Context;
use clap::{Args, Subcommand};
use std::path::{Path, PathBuf};

use crate::artifacts::{
    PublishContext, PublishDisposition, PublishOutcome, cloud::ArtifactUploadClient,
    paths::PublishRoots, publish,
};
use crate::config::Config;
use cas_store::{PublishedArtifact, SqliteArtifactStore};

#[derive(Debug, Clone, Subcommand)]
pub enum ArtifactCommands {
    /// Publish a local file as a durable artifact for a task
    Publish(PublishArgs),
    /// Show one artifact record
    Show(ShowArgs),
    /// List the artifacts published for a task
    List(ListArgs),
}

#[derive(Debug, Clone, Args)]
pub struct PublishArgs {
    /// Task the artifact belongs to
    #[arg(long)]
    pub task: String,
    /// File to publish. Must be under this task's artifacts directory or the checkout.
    pub path: PathBuf,
}

#[derive(Debug, Clone, Args)]
pub struct ShowArgs {
    /// Artifact record id, e.g. art-7f3a9c21
    pub id: String,
}

#[derive(Debug, Clone, Args)]
pub struct ListArgs {
    /// Task whose artifacts to list
    #[arg(long)]
    pub task: String,
}

pub fn execute(
    command: &ArtifactCommands,
    cli: &crate::cli::Cli,
    cas_root: &Path,
) -> anyhow::Result<()> {
    match command {
        ArtifactCommands::Publish(args) => execute_publish(args, cas_root, cli.json),
        ArtifactCommands::Show(args) => execute_show(args, cas_root, cli.json),
        ArtifactCommands::List(args) => execute_list(args, cas_root, cli.json),
    }
}

/// Build the upload client, or `None` when this installation has no
/// credentials. Absence is a supported state, not an error.
fn upload_client(cas_root: &Path) -> Option<ArtifactUploadClient> {
    crate::artifacts::cloud_client(cas_root)
}

fn execute_publish(args: &PublishArgs, cas_root: &Path, json: bool) -> anyhow::Result<()> {
    let store = SqliteArtifactStore::open(cas_root).context("opening the artifact store")?;
    let config = Config::load(cas_root).unwrap_or_default();
    let artifacts_root =
        crate::config::resolved_factory_artifacts_root(config.factory().artifacts_root.as_deref());
    let context = PublishContext {
        store: &store,
        roots: PublishRoots::new(cas_root, &artifacts_root, &args.task),
        task_id: args.task.clone(),
        cloud: upload_client(cas_root),
    };

    let outcome = publish(&context, &args.path).map_err(|error| anyhow::anyhow!("{error}"))?;

    if json {
        println!("{}", serde_json::to_string_pretty(&publish_json(&outcome))?);
        return Ok(());
    }
    print_publish(&outcome);
    Ok(())
}

fn publish_json(outcome: &PublishOutcome) -> serde_json::Value {
    let artifact = &outcome.artifact;
    let (storage, detail) = match &outcome.disposition {
        PublishDisposition::Committed { .. } => ("committed", None),
        PublishDisposition::StorageNotLive { reason } => ("not_live", Some(reason.clone())),
        PublishDisposition::NotLoggedIn => ("not_logged_in", None),
        PublishDisposition::UploadFailed { reason } => ("upload_failed", Some(reason.clone())),
    };
    serde_json::json!({
        "artifact_id": artifact.id,
        "task_id": artifact.task_id,
        "name": artifact.name,
        "mime": artifact.mime,
        "size_bytes": artifact.size_bytes,
        "sha256": artifact.sha256,
        "status": artifact.status,
        "storage": storage,
        "storage_detail": detail,
        "cloud_url": artifact.cloud_url,
        "source": outcome.source.display().to_string(),
    })
}

fn print_publish(outcome: &PublishOutcome) {
    let artifact = &outcome.artifact;
    println!(
        "[OK] published {} - {} - {}",
        artifact.name,
        human_size(artifact.size_bytes),
        artifact.id
    );
    println!("     {}", outcome.disposition.summary());
    println!("     sha256 {}", short_digest(&artifact.sha256));
    match &outcome.disposition {
        PublishDisposition::UploadFailed { reason } => {
            // The failure detail is the actionable part; it is printed in full,
            // indented, rather than truncated into the summary line.
            for line in reason.lines() {
                println!("     {line}");
            }
            println!("  -> cas artifact show {}", artifact.id);
        }
        PublishDisposition::StorageNotLive { .. } | PublishDisposition::NotLoggedIn => {
            println!("  -> cas artifact show {}", artifact.id);
        }
        PublishDisposition::Committed { .. } => {}
    }
}

fn execute_show(args: &ShowArgs, cas_root: &Path, json: bool) -> anyhow::Result<()> {
    let store = SqliteArtifactStore::open(cas_root).context("opening the artifact store")?;
    let Some(artifact) = store.get(&args.id)? else {
        anyhow::bail!("no artifact {}", args.id);
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&artifact)?);
        return Ok(());
    }
    println!(
        "[OK] {} - {} - {}",
        artifact.name,
        human_size(artifact.size_bytes),
        artifact.id
    );
    println!("     task    {}", artifact.task_id);
    println!("     type    {}", artifact.mime);
    println!("     sha256  {}", artifact.sha256);
    println!("     status  {}", artifact.status);
    if let Some(cloud_id) = &artifact.cloud_artifact_id {
        println!("     cloud   {cloud_id}");
    }
    if let Some(url) = &artifact.cloud_url {
        println!("     url     {url}");
    }
    if let Some(permalink) = &artifact.slack_permalink {
        println!("     slack   {permalink}");
    }
    println!("     created {}", artifact.created_at);
    Ok(())
}

fn execute_list(args: &ListArgs, cas_root: &Path, json: bool) -> anyhow::Result<()> {
    let store = SqliteArtifactStore::open(cas_root).context("opening the artifact store")?;
    let artifacts = store.list_for_task(&args.task)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&artifacts)?);
        return Ok(());
    }
    if artifacts.is_empty() {
        println!("[OK] no artifacts published for {}", args.task);
        println!("  -> cas artifact publish --task {} <path>", args.task);
        return Ok(());
    }
    println!("[OK] {} artifact(s) for {}", artifacts.len(), args.task);
    let width = artifacts
        .iter()
        .map(|artifact| artifact.name.chars().count())
        .max()
        .unwrap_or(0)
        .min(40);
    for artifact in &artifacts {
        println!(
            "     {:<width$}  {:>9}  {:<9}  {}",
            truncate(&artifact.name, width),
            human_size(artifact.size_bytes),
            artifact.status,
            artifact.id,
            width = width
        );
    }
    println!("  -> cas artifact show {}", artifacts[0].id);
    Ok(())
}

/// Decimal units, matching what a file dialog shows the operator.
fn human_size(bytes: u64) -> String {
    const KB: u64 = 1_000;
    const MB: u64 = 1_000_000;
    match bytes {
        bytes if bytes < KB => format!("{bytes} B"),
        bytes if bytes < MB => format!("{} KB", bytes.div_ceil(KB)),
        bytes => format!("{:.1} MB", bytes as f64 / MB as f64),
    }
}

/// Enough digest to compare by eye, in one terminal column.
fn short_digest(sha256: &str) -> String {
    if sha256.len() <= 16 {
        return sha256.to_string();
    }
    format!("{}…{}", &sha256[..8], &sha256[sha256.len() - 4..])
}

fn truncate(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_string();
    }
    let head: String = value.chars().take(width.saturating_sub(1)).collect();
    format!("{head}…")
}

/// Rendered by both the CLI and the MCP tool so the two surfaces cannot drift.
pub fn render_artifact_line(artifact: &PublishedArtifact) -> String {
    format!(
        "{} - {} - {} [{}]",
        artifact.name,
        human_size(artifact.size_bytes),
        artifact.id,
        artifact.status
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_render_in_the_units_a_file_dialog_uses() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(999), "999 B");
        assert_eq!(human_size(1_000), "1 KB");
        assert_eq!(human_size(812_345), "813 KB");
        assert_eq!(human_size(3_400_000), "3.4 MB");
    }

    #[test]
    fn a_digest_is_shortened_but_still_comparable() {
        let digest = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        let short = short_digest(digest);
        assert_eq!(short, "b94d27b9…cde9");
        assert!(digest.starts_with("b94d27b9") && digest.ends_with("cde9"));
        assert_eq!(short_digest("abc"), "abc", "a short value is left alone");
    }

    #[test]
    fn long_names_are_truncated_with_an_ellipsis_not_cut() {
        assert_eq!(truncate("short.pdf", 20), "short.pdf");
        let long = "a-very-long-artifact-name-indeed.pdf";
        let truncated = truncate(long, 10);
        assert_eq!(truncated.chars().count(), 10);
        assert!(truncated.ends_with('…'));
    }

    #[test]
    fn the_shared_line_names_the_facts_both_surfaces_must_agree_on() {
        let artifact = PublishedArtifact {
            id: "art-1".to_string(),
            task_id: "cas-b72a".to_string(),
            name: "brief.pdf".to_string(),
            mime: "application/pdf".to_string(),
            size_bytes: 812_345,
            sha256: "a".repeat(64),
            cloud_artifact_id: None,
            status: "local".to_string(),
            cloud_url: None,
            slack_permalink: None,
            created_at: "2026-09-18T20:00:00Z".to_string(),
        };
        let line = render_artifact_line(&artifact);
        assert!(line.contains("brief.pdf") && line.contains("art-1") && line.contains("local"));
        assert!(line.contains("813 KB"));
    }
}
