//! Changelog command.
//!
//! Fetches release notes from GitHub releases.

use std::time::Duration;

use anyhow::{Context, bail};
use clap::Args;
use serde::{Deserialize, Serialize};

use crate::cli::Cli;
use crate::ui::components::Formatter;
use crate::ui::theme::ActiveTheme;

const REPO_OWNER: &str = "Richards-LLC";
const REPO_NAME: &str = "cassy";
const RELEASES_URL: &str = "https://api.github.com/repos/Richards-LLC/cassy/releases";
const RELEASES_PAGE_SIZE: u8 = 50;
const API_TIMEOUT_MS: u64 = 8_000;
const MAX_LIMIT: u8 = 20;

#[derive(Args, Debug, Clone)]
pub struct ChangelogArgs {
    /// Number of releases to show
    #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u8).range(1..=MAX_LIMIT as i64))]
    pub limit: u8,

    /// Show a specific release by version/tag (e.g. 0.5.6 or v0.5.6)
    #[arg(long)]
    pub version: Option<String>,

    /// Include pre-releases in output
    #[arg(long)]
    pub include_prerelease: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct GitHubRelease {
    tag_name: String,
    name: Option<String>,
    body: Option<String>,
    html_url: String,
    published_at: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

pub fn execute(args: &ChangelogArgs, cli: &Cli) -> anyhow::Result<()> {
    let current_version = env!("CARGO_PKG_VERSION");

    let releases = if let Some(version) = &args.version {
        vec![fetch_release_by_tag(version, current_version)?]
    } else {
        fetch_releases(
            args.limit as usize,
            args.include_prerelease,
            current_version,
        )?
    };

    if cli.json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "source": format!("https://github.com/{}/{}/releases", REPO_OWNER, REPO_NAME),
                "count": releases.len(),
                "releases": releases,
            }))?
        );
        return Ok(());
    }

    if releases.is_empty() {
        let theme = ActiveTheme::default();
        let mut out = std::io::stdout();
        let mut fmt = Formatter::stdout(&mut out, theme);
        fmt.warning("No releases found.")?;
        return Ok(());
    }

    let theme = ActiveTheme::default();
    let mut out = std::io::stdout();
    let mut fmt = Formatter::stdout(&mut out, theme);

    fmt.subheading("CAS changelog")?;
    fmt.field(
        "Source",
        &format!("https://github.com/{REPO_OWNER}/{REPO_NAME}/releases"),
    )?;

    for (idx, release) in releases.iter().enumerate() {
        if idx > 0 {
            fmt.newline()?;
            fmt.write_muted(&"─".repeat(60))?;
            fmt.newline()?;
        }
        print_release(release, &mut fmt)?;
    }

    Ok(())
}

fn fetch_releases(
    limit: usize,
    include_prerelease: bool,
    current_version: &str,
) -> anyhow::Result<Vec<GitHubRelease>> {
    let url = format!("{RELEASES_URL}?per_page={RELEASES_PAGE_SIZE}");
    let response = github_get(&url, current_version)
        .with_context(|| format!("Failed to fetch releases from {url}"))?;

    let mut releases: Vec<GitHubRelease> = response
        .into_json()
        .context("Failed to parse GitHub releases response")?;

    releases.retain(|r| !r.draft && (include_prerelease || !r.prerelease));
    releases.truncate(limit);
    Ok(releases)
}

fn fetch_release_by_tag(tag: &str, current_version: &str) -> anyhow::Result<GitHubRelease> {
    let normalized = normalize_tag(tag);
    let url = format!("{RELEASES_URL}/tags/{normalized}");
    let response = match ureq::AgentBuilder::new()
        .timeout(Duration::from_millis(API_TIMEOUT_MS))
        .build()
        .get(&url)
        .set("Accept", "application/vnd.github+json")
        .set("User-Agent", &format!("cas/{current_version}"))
        .call()
    {
        Ok(response) => response,
        Err(ureq::Error::Status(404, _)) => {
            bail!(
                "Release '{}' not found. Try `cas changelog` to list available versions.",
                normalized,
            );
        }
        Err(e) => {
            return Err(anyhow::Error::new(e))
                .with_context(|| format!("Failed to fetch release {normalized}"));
        }
    };

    let release: GitHubRelease = response
        .into_json()
        .context("Failed to parse GitHub release response")?;

    if release.draft {
        bail!("Release '{normalized}' is a draft and not publicly available.");
    }

    Ok(release)
}

fn github_get(url: &str, current_version: &str) -> anyhow::Result<ureq::Response> {
    let response = ureq::AgentBuilder::new()
        .timeout(Duration::from_millis(API_TIMEOUT_MS))
        .build()
        .get(url)
        .set("Accept", "application/vnd.github+json")
        .set("User-Agent", &format!("cas/{current_version}"))
        .call()?;
    Ok(response)
}

fn normalize_tag(version_or_tag: &str) -> String {
    let trimmed = version_or_tag.trim();
    if trimmed.starts_with('v') {
        trimmed.to_string()
    } else {
        format!("v{trimmed}")
    }
}

fn print_release(release: &GitHubRelease, fmt: &mut Formatter) -> std::io::Result<()> {
    let title = release
        .name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(&release.tag_name);

    fmt.newline()?;
    fmt.write_accent(&release.tag_name)?;
    fmt.write_raw(" ")?;
    fmt.write_bold(title)?;
    fmt.newline()?;

    if let Some(published_at) = &release.published_at {
        fmt.field("Published", published_at)?;
    }
    if release.prerelease {
        fmt.warning("Prerelease")?;
    }
    fmt.write_accent(&release.html_url)?;
    fmt.newline()?;

    let body = release
        .body
        .as_deref()
        .map(str::trim)
        .filter(|body| !body.is_empty())
        .unwrap_or("No release notes provided.");

    fmt.newline()?;
    fmt.write_raw(body)?;
    fmt.newline()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_tag_adds_v_prefix() {
        assert_eq!(normalize_tag("0.5.6"), "v0.5.6");
    }

    #[test]
    fn normalize_tag_keeps_v_prefix() {
        assert_eq!(normalize_tag("v0.5.6"), "v0.5.6");
    }
}
