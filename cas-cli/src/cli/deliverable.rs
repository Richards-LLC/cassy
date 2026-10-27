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
use crate::deliverable::capture::{self, CaptureRequest, Side};
use crate::deliverable::finalize::{self, Budget, FinalizeRequest, Paper};
use crate::deliverable::pairs::{self, PairsRequest, Selection};
use crate::deliverable::{Audience, Receipt, UsageError};
use crate::ui::components::{Formatter, Verdict};
use crate::ui::theme::ActiveTheme;

#[derive(Subcommand)]
pub enum DeliverableCommands {
    /// Resolve the project's brand (tokens, type, mark) from its DESIGN.md
    Brand(BrandArgs),
    /// Turn a draft HTML into the branded, single-file deliverable and its PDFs
    Finalize(FinalizeArgs),
    /// Capture real merge-base/head evidence in one bounded QA run
    Capture(CaptureArgs),
    /// List complete, verified pairs for a task or commit range
    Pairs(PairsArgs),
}

#[derive(Args)]
pub struct CaptureArgs {
    #[arg(long)]
    pub task: String,
    /// Both revisions by default; collect one side explicitly for a deployment
    #[arg(long, value_enum, default_value_t = Side::Both)]
    pub side: Side,
    #[arg(long, default_value = ".")]
    pub root: PathBuf,
    /// Override the durable qa/pairs output directory
    #[arg(long)]
    pub dir: Option<PathBuf>,
    /// Explicit integration target; otherwise read the task's declared target
    #[arg(long)]
    pub target: Option<String>,
    /// Project-relative shot catalog; otherwise use qa.capture.shots
    #[arg(long)]
    pub shots: Option<PathBuf>,
}

#[derive(Args)]
pub struct PairsArgs {
    #[arg(long, required_unless_present = "range", conflicts_with = "range")]
    pub task: Option<String>,
    /// BASE..HEAD, joined to tasks through recorded commit provenance
    #[arg(long)]
    pub range: Option<String>,
    #[arg(long, default_value = ".")]
    pub root: PathBuf,
    #[arg(long, requires = "task")]
    pub dir: Option<PathBuf>,
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

#[derive(Args)]
pub struct FinalizeArgs {
    /// Draft HTML holding the /*cas:brand*/ marker; relative images resolve beside it
    pub draft: PathBuf,
    /// Who reads it; sets the brand rule, embed budget and hygiene
    #[arg(long, value_enum)]
    pub audience: Audience,
    /// Single-file HTML to write [default: <draft stem>-final.html]
    #[arg(long)]
    pub out: Option<PathBuf>,
    /// Project root whose DESIGN.md brands the deliverable
    #[arg(long, default_value = ".")]
    pub root: PathBuf,
    /// File or directory the deliverable shows; its nearest DESIGN.md wins
    #[arg(long)]
    pub surface: Option<PathBuf>,
    /// Paper formats to print and check, e.g. a4,letter
    #[arg(long, value_enum, value_delimiter = ',')]
    pub pdf: Vec<Paper>,
    /// PDF Author metadata
    #[arg(long)]
    pub author: Option<String>,
    /// A word that is the client's own and passes hygiene (repeatable)
    #[arg(long)]
    pub allow: Vec<String>,
    /// Keep check-pdf page PNGs and receipts here, one folder per format
    #[arg(long)]
    pub check_dir: Option<PathBuf>,
    /// Per-image embed limit in KB [default: 120]
    #[arg(long)]
    pub max_image_kb: Option<u64>,
    /// Report-wide image limit in KB [default: 1440 internal, 2880 customer]
    #[arg(long)]
    pub max_images_kb: Option<u64>,
}

pub fn execute(command: &DeliverableCommands, cli: &Cli) -> anyhow::Result<()> {
    match command {
        DeliverableCommands::Brand(args) => execute_brand(args, cli),
        DeliverableCommands::Finalize(args) => execute_finalize(args, cli),
        DeliverableCommands::Capture(args) => execute_capture(args, cli),
        DeliverableCommands::Pairs(args) => execute_pairs(args, cli),
    }
}

fn capture_root(root: &std::path::Path) -> anyhow::Result<(PathBuf, PathBuf)> {
    let root = root
        .canonicalize()
        .map_err(|_| UsageError("--root is not a readable project".into()))?;
    let store = crate::store::CasContext::from_path(&root)
        .map(|context| context.cas_root)
        .unwrap_or_else(|_| root.join(".cas"));
    Ok((root, store))
}

fn execute_capture(args: &CaptureArgs, cli: &Cli) -> anyhow::Result<()> {
    let (root, cas_root) = capture_root(&args.root)?;
    let receipt = capture::capture(&CaptureRequest {
        root,
        cas_root,
        task: args.task.clone(),
        side: args.side,
        dir: args.dir.as_ref().map(std::path::absolute).transpose()?,
        target: args.target.clone(),
        shots: args.shots.clone(),
    })?;
    emit(cli, &receipt, |fmt| {
        fmt.verdict(
            if receipt.findings.is_empty() {
                Verdict::Ok
            } else {
                Verdict::Error
            },
            if receipt.findings.is_empty() {
                "capture collected"
            } else {
                "capture unavailable"
            },
            &format!(
                "{} pair(s) · {} unpaired · {:.1} s",
                receipt.pairs,
                receipt.unpaired,
                receipt.elapsed_ms as f64 / 1000.0
            ),
        )?;
        fmt.field(
            "  Evidence",
            &format!("{}/qa/pairs/pairs.json", receipt.task_id),
        )?;
        fmt.field("  Full paths", "--json")
    })
}

fn execute_pairs(args: &PairsArgs, cli: &Cli) -> anyhow::Result<()> {
    let (root, cas_root) = capture_root(&args.root)?;
    let selection = match (&args.task, &args.range) {
        (Some(task), None) => Selection::Task(task.clone()),
        (None, Some(range)) => Selection::Range(range.clone()),
        _ => return Err(UsageError("choose --task or --range".into()).into()),
    };
    let receipt = pairs::pairs(&PairsRequest {
        root,
        cas_root,
        selection,
        dir: args.dir.as_ref().map(std::path::absolute).transpose()?,
    })?;
    emit(cli, &receipt, |fmt| {
        fmt.verdict(
            if receipt.findings.is_empty() {
                Verdict::Ok
            } else {
                Verdict::Error
            },
            if receipt.findings.is_empty() {
                "pairs verified"
            } else {
                "incomplete evidence"
            },
            &format!(
                "{} pair(s) · {} unpaired",
                receipt.pairs.len(),
                receipt.unpaired.len()
            ),
        )?;
        fmt.field("  Full paths", "--json")
    })
}

fn execute_finalize(args: &FinalizeArgs, cli: &Cli) -> anyhow::Result<()> {
    let root = args
        .root
        .canonicalize()
        .map_err(|_| UsageError(format!("--root {} is not a directory", args.root.display())))?;
    let out = args.out.clone().unwrap_or_else(|| {
        let stem = args
            .draft
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("report");
        args.draft.with_file_name(format!("{stem}-final.html"))
    });
    let mut budget = Budget::for_audience(args.audience);
    if let Some(kb) = args.max_image_kb {
        budget.image_bytes = kb * 1024;
    }
    if let Some(kb) = args.max_images_kb {
        budget.images_bytes = kb * 1024;
    }
    let receipt = finalize::finalize(&FinalizeRequest {
        draft: args.draft.clone(),
        out,
        audience: args.audience,
        root,
        surface: args.surface.clone(),
        tokens: None,
        allow: args.allow.clone(),
        budget,
        papers: args.pdf.clone(),
        pdf_dir: None,
        check_dir: args.check_dir.clone(),
        author: args.author.clone(),
    })?;
    emit(cli, &receipt, |fmt| {
        let verdict = match (receipt.findings.is_empty(), receipt.warnings.is_empty()) {
            (false, _) => (Verdict::Error, "not deliverable"),
            (true, true) => (Verdict::Ok, "deliverable ready"),
            (true, false) => (Verdict::Warning, "deliverable ready"),
        };
        let kb = receipt.bytes.div_ceil(1024);
        fmt.verdict(
            verdict.0,
            verdict.1,
            &format!(
                "{} · {kb} KB · {} image(s) inlined",
                receipt.audience.as_str(),
                receipt.inlined
            ),
        )?;
        if let Some(html) = &receipt.html {
            fmt.field("  HTML", html)?;
        }
        if let Some(design) = &receipt.design {
            fmt.field("  Design", design)?;
        }
        for pdf in &receipt.pdf {
            let pages = pdf
                .pages
                .map_or_else(|| "?".to_string(), |pages| pages.to_string());
            fmt.field(
                &format!("  {}", pdf.paper.as_str()),
                &format!("{} · {pages} pages", pdf.path),
            )?;
        }
        Ok(())
    })
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
