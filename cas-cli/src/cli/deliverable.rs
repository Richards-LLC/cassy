//! `cas deliverable`: the branded-deliverables core verbs.
//!
//! Every verb prints one receipt (`--json`: one JSON document) and exits
//! 0 on pass, 1 on a finding, 2 on a usage or environment error.

use std::fs;
use std::io;
use std::path::PathBuf;

use clap::{Args, Subcommand};

use crate::cli::{Cli, JsonReportedFailure};
use crate::deliverable::brand::{self, BrandRequest};
use crate::deliverable::{Audience, Receipt, UsageError};
use crate::ui::components::{Formatter, Verdict};
use crate::ui::theme::ActiveTheme;

#[derive(Subcommand)]
pub enum DeliverableCommands {
    /// Resolve the project's brand (tokens, type, mark) from its DESIGN.md
    Brand(BrandArgs),
}

#[derive(Args)]
pub struct BrandArgs {
    /// Project root to read
    #[arg(long, default_value = ".")]
    pub root: PathBuf,
    /// Who reads it: customer output refuses the bundled default brand
    #[arg(long, value_enum, default_value_t = Audience::Internal)]
    pub audience: Audience,
    /// File or directory the deliverable shows; its nearest DESIGN.md wins
    #[arg(long)]
    pub surface: Option<PathBuf>,
    /// Design-tokens JSON that overrides every DESIGN.md
    #[arg(long)]
    pub tokens: Option<PathBuf>,
    /// Write the resolved CSS to this file
    #[arg(long)]
    pub css_out: Option<PathBuf>,
}

pub fn execute(command: &DeliverableCommands, cli: &Cli) -> anyhow::Result<()> {
    match command {
        DeliverableCommands::Brand(args) => execute_brand(args, cli),
    }
}

fn execute_brand(args: &BrandArgs, cli: &Cli) -> anyhow::Result<()> {
    let root = args
        .root
        .canonicalize()
        .map_err(|_| UsageError(format!("--root {} is not a directory", args.root.display())))?;
    let resolved = brand::resolve(&BrandRequest {
        root,
        audience: args.audience,
        surface: args.surface.clone(),
        tokens: args.tokens.clone(),
    })?;
    let mut receipt = resolved.receipt.clone();
    if let (Some(path), Some(css)) = (&args.css_out, resolved.css()) {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, css)
            .map_err(|error| UsageError(format!("--css-out {}: {error}", path.display())))?;
        receipt.css_out = Some(path.display().to_string());
    }
    emit(cli, &receipt, |fmt| {
        let verdict = match (receipt.findings.is_empty(), receipt.warnings.is_empty()) {
            (false, _) => (Verdict::Error, "no brand"),
            (true, true) => (Verdict::Ok, "brand resolved"),
            (true, false) => (Verdict::Warning, "brand resolved"),
        };
        let source = receipt.source.as_deref().unwrap_or("refused");
        fmt.verdict(
            verdict.0,
            verdict.1,
            &format!("{source} · {}", receipt.audience.as_str()),
        )?;
        if let Some(design) = receipt.design_line() {
            fmt.field("  Design", &design)?;
        }
        if let Some(mark) = receipt
            .mark
            .as_ref()
            .and_then(|mark| mark.get("path"))
            .and_then(|p| p.as_str())
        {
            fmt.field("  Mark", mark)?;
        }
        if let Some(path) = &receipt.css_out {
            fmt.field("  CSS", path)?;
        }
        Ok(())
    })
}

/// Print a verb's receipt and apply the exit contract.
fn emit<R: Receipt>(
    cli: &Cli,
    receipt: &R,
    human: impl FnOnce(&mut Formatter<'_>) -> io::Result<()>,
) -> anyhow::Result<()> {
    if cli.json {
        println!("{}", serde_json::to_string_pretty(receipt)?);
    } else {
        let stdout = io::stdout();
        let mut output = stdout.lock();
        let mut fmt = Formatter::stdout(&mut output, ActiveTheme::default());
        human(&mut fmt)?;
        for finding in receipt.findings() {
            fmt.bullet(&format!("finding: {} ({})", finding.message, finding.code))?;
        }
        for warning in receipt.warnings() {
            fmt.bullet(&format!("warning: {warning}"))?;
        }
        fmt.flush()?;
    }
    if receipt.findings().is_empty() {
        return Ok(());
    }
    let failure = anyhow::anyhow!(
        "{} finding{}",
        receipt.findings().len(),
        if receipt.findings().len() == 1 {
            ""
        } else {
            "s"
        }
    );
    Err(if cli.json {
        JsonReportedFailure(failure).into()
    } else {
        failure
    })
}
