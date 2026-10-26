//! `finalize`: a front-end's draft HTML becomes the deliverable.
//!
//! One call, in order: fill the draft's brand markers ([`super::brand`]),
//! inline its local images within the audience's budget (`assets/inline.py`),
//! check the visible text for the audience (`assets/hygiene.py`), then, when
//! paper formats are asked for, print each (`assets/print.cjs`) and check it
//! (`assets/check-pdf.py`). Every step is lifted from the release report or
//! the SOW report; their skills keep no copy.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use super::asset::{self, Asset, Workspace};
use super::brand::{self, BrandRequest};
use super::{Audience, Finding, Receipt, UsageError};

const INLINE: Asset = Asset {
    path: "inline.py",
    content: include_str!("assets/inline.py"),
};
const HYGIENE: Asset = Asset {
    path: "hygiene.py",
    content: include_str!("assets/hygiene.py"),
};
const PRINT: Asset = Asset {
    path: "print.cjs",
    content: include_str!("assets/print.cjs"),
};
const CHECK_PDF: Asset = Asset {
    path: "check-pdf.py",
    content: include_str!("assets/check-pdf.py"),
};
const SCRIPT_TIMEOUT: Duration = Duration::from_secs(60);
const PDF_TIMEOUT: Duration = Duration::from_secs(180);
const KB: u64 = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, clap::ValueEnum)]
pub enum Paper {
    #[value(name = "a4")]
    A4,
    #[value(name = "letter")]
    Letter,
}

impl Paper {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::A4 => "A4",
            Self::Letter => "Letter",
        }
    }
}

/// Embed limits. Images are budgeted per image and across the report;
/// overflow drops whole pairs, lowest priority first, and names them.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    pub image_bytes: u64,
    pub images_bytes: u64,
    pub file_bytes: u64,
}

impl Budget {
    /// About 120 KB per side for every audience; an internal report carries
    /// up to six pairs, a customer report up to twelve; 8 MB per file.
    pub fn for_audience(audience: Audience) -> Self {
        let pairs = match audience {
            Audience::Internal => 6,
            Audience::Customer => 12,
        };
        Self {
            image_bytes: 120 * KB,
            images_bytes: pairs * 2 * 120 * KB,
            file_bytes: 8 * KB * KB,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FinalizeRequest {
    /// The draft HTML; its relative image paths resolve beside it.
    pub draft: PathBuf,
    /// Where the single-file HTML is written.
    pub out: PathBuf,
    pub audience: Audience,
    /// The project whose DESIGN.md brands the deliverable.
    pub root: PathBuf,
    pub surface: Option<PathBuf>,
    pub tokens: Option<PathBuf>,
    /// Words that are the client's own and pass hygiene.
    pub allow: Vec<String>,
    pub budget: Budget,
    /// Paper formats to print; empty prints nothing.
    pub papers: Vec<Paper>,
    /// Where `<out stem>-<paper>.pdf` files go (default: beside `out`).
    pub pdf_dir: Option<PathBuf>,
    /// Where check-pdf writes page PNGs and its receipt (default: discarded).
    pub check_dir: Option<PathBuf>,
    pub author: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PdfReceipt {
    pub paper: Paper,
    pub path: String,
    pub pages: Option<u64>,
    /// check-pdf's own receipt: bounds findings, links, local links.
    pub check: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct FinalizeReceipt {
    pub schema: &'static str,
    pub audience: Audience,
    pub draft: String,
    /// The written HTML; absent when a finding stopped before writing it.
    pub html: Option<String>,
    pub bytes: u64,
    pub image_bytes: u64,
    pub brand_source: Option<String>,
    /// `path@revision` of the DESIGN.md worn.
    pub design: Option<String>,
    pub inlined: usize,
    /// Image units left out to meet the budget, each with its reason.
    pub dropped: Vec<Value>,
    pub pdf: Vec<PdfReceipt>,
    pub warnings: Vec<String>,
    pub findings: Vec<Finding>,
}

impl Receipt for FinalizeReceipt {
    fn findings(&self) -> &[Finding] {
        &self.findings
    }
    fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

pub fn finalize(request: &FinalizeRequest) -> anyhow::Result<FinalizeReceipt> {
    // Scripts run from the draft's directory; every path they get is absolute.
    let mut request = request.clone();
    for path in [&mut request.draft, &mut request.out] {
        *path = std::path::absolute(&*path)?;
    }
    for dir in [&mut request.pdf_dir, &mut request.check_dir]
        .into_iter()
        .flatten()
    {
        *dir = std::path::absolute(&*dir)?;
    }
    let request = &request;
    let draft = fs::read_to_string(&request.draft)
        .map_err(|error| UsageError(format!("draft {}: {error}", request.draft.display())))?;
    let brand = brand::resolve(&BrandRequest {
        root: request.root.clone(),
        audience: request.audience,
        surface: request.surface.clone(),
        tokens: request.tokens.clone(),
    })?;
    let mut receipt = FinalizeReceipt {
        schema: "cas.deliverable.finalize/1",
        audience: request.audience,
        draft: request.draft.display().to_string(),
        html: None,
        bytes: 0,
        image_bytes: 0,
        brand_source: brand.receipt.source.clone(),
        design: brand.receipt.design_line(),
        inlined: 0,
        dropped: Vec::new(),
        pdf: Vec::new(),
        warnings: brand.receipt.warnings.clone(),
        findings: brand.receipt.findings.clone(),
    };
    if brand.css().is_none() {
        return Ok(receipt);
    }
    // An internal HTML finalized once already keeps the brand it wears, so
    // its PDFs can be printed and checked again; a customer draft must carry
    // the marker so it never ships without the project's brand.
    let filled = if !draft.contains(brand::STYLE_MARKER) && request.audience == Audience::Internal {
        receipt
            .warnings
            .push("draft carries no brand marker; its own styles are kept".to_string());
        draft
    } else {
        brand
            .fill(&draft)
            .map_err(|error| UsageError(format!("draft {}: {error}", request.draft.display())))?
    };

    let workspace = Workspace::materialize(&[INLINE, HYGIENE, PRINT, CHECK_PDF])?;
    let staged = workspace.path("filled.html");
    fs::write(&staged, filled)?;
    let base = request
        .draft
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .to_path_buf();
    let inlined = workspace.python(
        INLINE.path,
        [
            staged.as_os_str().to_owned(),
            request.out.as_os_str().to_owned(),
            "--base".into(),
            base.as_os_str().to_owned(),
            "--max-image-bytes".into(),
            request.budget.image_bytes.to_string().into(),
            "--max-images-bytes".into(),
            request.budget.images_bytes.to_string().into(),
            "--max-file-bytes".into(),
            request.budget.file_bytes.to_string().into(),
        ],
        &base,
        SCRIPT_TIMEOUT,
    )?;
    receipt.html = Some(request.out.display().to_string());
    receipt.bytes = inlined["bytes"].as_u64().unwrap_or(0);
    receipt.image_bytes = inlined["image_bytes"].as_u64().unwrap_or(0);
    receipt.inlined = inlined["inlined"].as_array().map_or(0, Vec::len);
    receipt.dropped = inlined["dropped"].as_array().cloned().unwrap_or_default();
    receipt.warnings.extend(strings(&inlined["warnings"]));
    receipt.findings.extend(findings(&inlined["findings"]));

    let mut args = vec![
        request.out.as_os_str().to_owned(),
        "--audience".into(),
        request.audience.as_str().into(),
    ];
    for word in &request.allow {
        args.extend(["--allow".into(), word.into()]);
    }
    let hygiene = workspace.python(HYGIENE.path, args, &base, SCRIPT_TIMEOUT)?;
    receipt.findings.extend(findings(&hygiene["findings"]));

    if request.papers.is_empty() {
        return Ok(receipt);
    }
    if !receipt.findings.is_empty() {
        receipt
            .warnings
            .push("not printed: resolve the findings first".to_string());
        return Ok(receipt);
    }
    require_print_tools()?;
    let pdf_dir = request.pdf_dir.clone().unwrap_or_else(|| {
        request
            .out
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf()
    });
    let printed = print(&workspace, request, &pdf_dir)?;
    if let Some(blocked) = printed {
        receipt.findings.push(Finding {
            code: "network-request".to_string(),
            message: blocked,
        });
    }
    let stem = request
        .out
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("report");
    for paper in &request.papers {
        let pdf = pdf_dir.join(format!("{stem}-{}.pdf", paper.as_str()));
        if !pdf.is_file() {
            return Err(UsageError(format!(
                "print completed without creating {}",
                pdf.display()
            ))
            .into());
        }
        let check_dir = match &request.check_dir {
            Some(dir) => dir.join(paper.as_str()),
            None => workspace.path(&format!("check-{}", paper.as_str())),
        };
        let mut command = Command::new("python3");
        command
            .current_dir(&base)
            .arg("-I")
            .arg(workspace.path(CHECK_PDF.path))
            .arg(&pdf)
            .arg(&check_dir);
        let output = asset::run(&mut command, SCRIPT_TIMEOUT, "python3")?;
        let check: Value = String::from_utf8_lossy(&output.stdout)
            .lines()
            .rev()
            .find_map(|line| serde_json::from_str(line).ok())
            .unwrap_or(Value::Null);
        if !output.status.success() {
            receipt.findings.push(Finding {
                code: "check-pdf".to_string(),
                message: format!(
                    "{} {}: {}",
                    paper.as_str(),
                    pdf.display(),
                    asset::last_line(&output)
                ),
            });
        }
        receipt.pdf.push(PdfReceipt {
            paper: *paper,
            path: pdf.display().to_string(),
            pages: check["pages"].as_u64(),
            check,
        });
    }
    Ok(receipt)
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn findings(value: &Value) -> Vec<Finding> {
    serde_json::from_value(value.clone()).unwrap_or_default()
}

/// Printing needs Node and checking needs PyMuPDF; say so before printing.
fn require_print_tools() -> anyhow::Result<()> {
    let node = Command::new("node").arg("--version").output();
    if !node.as_ref().is_ok_and(|output| output.status.success()) {
        return Err(
            UsageError("print: Node.js is not installed; install Node.js 18+".to_string()).into(),
        );
    }
    let pymupdf = Command::new("python3")
        .args(["-c", "import pymupdf"])
        .output();
    if !pymupdf.as_ref().is_ok_and(|output| output.status.success()) {
        return Err(UsageError(
            "check-pdf: PyMuPDF is not installed; run `python3 -m pip install pymupdf`".to_string(),
        )
        .into());
    }
    Ok(())
}

/// Print every requested paper; `Some(message)` when the page reached for
/// the network. Without a resolvable Playwright module, installs one into the
/// disposable workspace (npm exec does not expose it to an external script).
fn print(
    workspace: &Workspace,
    request: &FinalizeRequest,
    pdf_dir: &Path,
) -> anyhow::Result<Option<String>> {
    let papers: Vec<&str> = request.papers.iter().map(|paper| paper.as_str()).collect();
    let command = |module: Option<&Path>| {
        let mut command = Command::new("node");
        command
            .arg(workspace.path(PRINT.path))
            .arg(&request.out)
            .arg(pdf_dir)
            .arg(papers.join(","));
        if let Some(author) = &request.author {
            command.arg(author);
        }
        if let Some(module) = module {
            command
                .env("NODE_PATH", module.parent().unwrap_or(module))
                .env("PLAYWRIGHT_MODULE", module);
        }
        command
    };
    let mut output = asset::run(&mut command(None), PDF_TIMEOUT, "node")?;
    if !output.status.success() && playwright_module_missing(&asset::last_line(&output)) {
        let mut install = Command::new("npm");
        install
            .current_dir(workspace.path(""))
            .args(["install", "--no-save", "playwright"]);
        let installed = asset::run(&mut install, PDF_TIMEOUT, "npm install")?;
        if !installed.status.success() {
            return Err(UsageError(format!(
                "print: Playwright unavailable: {}; remedy `npm install --no-save playwright`",
                asset::last_line(&installed)
            ))
            .into());
        }
        let module = workspace.path("node_modules/playwright");
        output = asset::run(&mut command(Some(&module)), PDF_TIMEOUT, "node")?;
    }
    match output.status.code() {
        Some(0) => Ok(None),
        Some(3) => Ok(Some(asset::last_line(&output))),
        _ => Err(UsageError(format!("print failed: {}", asset::last_line(&output))).into()),
    }
}

fn playwright_module_missing(detail: &str) -> bool {
    let detail = detail.to_ascii_lowercase();
    detail.contains("playwright is not installed")
        || detail.contains("cannot find module 'playwright'")
        || detail.contains("cannot find module \"playwright\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playwright_module_failure_is_selected_for_npm_fallback() {
        assert!(playwright_module_missing(
            "Playwright is not installed: Cannot find module 'playwright'"
        ));
        assert!(playwright_module_missing(
            "Error: Cannot find module \"playwright\""
        ));
        assert!(!playwright_module_missing(
            "browserType.launch: Executable doesn't exist"
        ));
    }
}
