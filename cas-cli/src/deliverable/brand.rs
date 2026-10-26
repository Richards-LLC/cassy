//! `brand`: the project's tokens, type and mark for a deliverable.
//!
//! Resolution lives in the embedded `assets/brand.py`, lifted from the
//! release report's renderer; see its docstring for the order (PROFILE,
//! nearest-surface DESIGN.md, root, single nested, `inherits:`; CSS sources
//! before design-spec value keys). Renderers leave [`STYLE_MARKER`] and
//! [`MARK_MARKER`] in their HTML and [`Brand::fill`] replaces them.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::asset::{Asset, Workspace};
use super::{Audience, Finding, Receipt, UsageError};
use crate::bounded_process::{BoundedCommandError, Deadline, run_command};

const SCRIPT: Asset = Asset {
    path: "brand.py",
    content: include_str!("assets/brand.py"),
};
const DEFAULT_TOKENS: &str = "skills/design-spec/references/design-tokens.json";
const TIMEOUT: Duration = Duration::from_secs(60);

/// Where a renderer's `<style>` takes the brand CSS.
pub const STYLE_MARKER: &str = "/*cas:brand*/";
/// Where a renderer's lockup takes the brand mark.
pub const MARK_MARKER: &str = "<!--cas:brand-mark-->";

#[derive(Debug, Clone)]
pub struct BrandRequest {
    pub root: PathBuf,
    pub audience: Audience,
    /// A file or directory the deliverable shows; its nearest DESIGN.md wins.
    pub surface: Option<PathBuf>,
    /// An explicit design-tokens JSON that overrides every DESIGN.md.
    pub tokens: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrandReceipt {
    pub schema: String,
    pub audience: Audience,
    pub root: String,
    /// tokens | design-md-css | design-md-values | design-tokens-json | defaults;
    /// absent when a finding refused the brand.
    pub source: Option<String>,
    /// The DESIGN.md worn: path, revision, how it was found, its inherits chain.
    pub design: Option<Value>,
    pub tokens: Option<Value>,
    /// Display, body and mono stacks, naming the fallback when no face is local.
    #[serde(rename = "type")]
    pub type_: Option<Value>,
    pub mark: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub css_out: Option<String>,
    #[serde(default)]
    pub css_bytes: usize,
    pub warnings: Vec<String>,
    pub findings: Vec<Finding>,
}

impl Receipt for BrandReceipt {
    fn findings(&self) -> &[Finding] {
        &self.findings
    }
    fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

impl BrandReceipt {
    /// `path@revision` of the DESIGN.md worn, the line a task records.
    pub fn design_line(&self) -> Option<String> {
        let design = self.design.as_ref()?;
        let path = design.get("path")?.as_str()?;
        Some(match design.get("revision").and_then(Value::as_str) {
            Some(revision) => format!("{path}@{revision}"),
            None => format!("{path}@uncommitted"),
        })
    }
}

/// A resolved brand: the receipt plus the CSS and mark it describes.
#[derive(Debug, Clone)]
pub struct Brand {
    pub receipt: BrandReceipt,
    css: Option<String>,
    mark_uri: Option<String>,
}

#[derive(Deserialize)]
struct ScriptOutput {
    receipt: BrandReceipt,
    css: Option<String>,
    mark_uri: Option<String>,
}

pub fn resolve(request: &BrandRequest) -> anyhow::Result<Brand> {
    if !request.root.is_dir() {
        return Err(UsageError(format!(
            "--root {} is not a directory",
            request.root.display()
        ))
        .into());
    }
    if let Some(tokens) = &request.tokens
        && !tokens.is_file()
    {
        return Err(UsageError(format!("--tokens {} does not exist", tokens.display())).into());
    }
    let workspace = Workspace::materialize(&[
        SCRIPT,
        Asset::builtin(DEFAULT_TOKENS, "default-tokens.json")?,
    ])?;
    let mut command = Command::new("python3");
    command
        .current_dir(&request.root)
        .arg("-I")
        .arg(workspace.path(SCRIPT.path))
        .arg("--root")
        .arg(&request.root)
        .arg("--defaults")
        .arg(workspace.path("default-tokens.json"))
        .arg("--audience")
        .arg(request.audience.as_str());
    if let Some(tokens) = &request.tokens {
        command.arg("--tokens").arg(tokens);
    }
    if let Some(surface) = &request.surface {
        command.arg("--surface").arg(surface);
    }
    let output = run_command(&mut command, Deadline::after(TIMEOUT), TIMEOUT).map_err(|error| {
        UsageError(match error {
            BoundedCommandError::TimedOut => "brand: python3 timed out".to_string(),
            BoundedCommandError::Io => {
                "brand: python3 is unavailable; install Python 3".to_string()
            }
        })
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("no detail");
        return Err(UsageError(format!("brand: {}", detail.trim_start_matches("brand: "))).into());
    }
    let parsed: ScriptOutput = serde_json::from_slice(&output.stdout)
        .map_err(|error| UsageError(format!("brand: unreadable resolver output: {error}")))?;
    let mut receipt = parsed.receipt;
    receipt.css_bytes = parsed.css.as_deref().map_or(0, str::len);
    Ok(Brand {
        receipt,
        css: parsed.css,
        mark_uri: parsed.mark_uri,
    })
}

impl Brand {
    /// The resolved CSS; `None` when a finding refused the brand.
    pub fn css(&self) -> Option<&str> {
        self.css.as_deref()
    }

    /// Replace a renderer's brand markers with this brand's CSS and mark.
    pub fn fill(&self, html: &str) -> anyhow::Result<String> {
        let css = self.css().ok_or_else(|| {
            let reasons: Vec<&str> = self
                .receipt
                .findings
                .iter()
                .map(|f| f.message.as_str())
                .collect();
            anyhow::anyhow!("no brand to fill: {}", reasons.join("; "))
        })?;
        if html.matches(STYLE_MARKER).count() != 1 || html.matches(MARK_MARKER).count() > 1 {
            anyhow::bail!(
                "HTML needs exactly one {STYLE_MARKER} brand marker and at most one {MARK_MARKER}"
            );
        }
        let mark = self
            .mark_uri
            .as_deref()
            .map(|uri| format!(r#"<img class="mark" src="{uri}" alt="" width="32" height="32">"#))
            .unwrap_or_default();
        Ok(html
            .replacen(STYLE_MARKER, css, 1)
            .replacen(MARK_MARKER, &mark, 1))
    }
}
