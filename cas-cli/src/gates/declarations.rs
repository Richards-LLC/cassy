//! Shared build-time and merge-publication gate registry validation.
use std::{collections::BTreeMap, fs, path::Path};

pub(crate) struct GateDeclaration {
    pub family: char,
    pub number: u32,
    pub code: String,
    pub stage: String,
    pub title: String,
    pub source: String,
    pub policy: Option<String>,
}

/// Repository-relative roots; workflow declarations live in scripts.
pub(crate) const SCAN_ROOTS: &[&str] = &["cas-cli/src", "crates", "scripts"];
pub(crate) const DELIVERY_STAGES: &[&str] = &["park", "close-after-merge", "epic-close", "merge-admission", "train-merge"];

pub(crate) fn scan(repo_root: &Path) -> Result<Vec<GateDeclaration>, String> {
    let mut rows = Vec::new();
    let mut seen = BTreeMap::new();
    for root in SCAN_ROOTS {
        let root = repo_root.join(root);
        if root.is_dir() {
            collect(&root, repo_root, &mut rows, &mut seen)?;
        }
    }
    rows.sort_by_key(|row| ("GHWNC".find(row.family).unwrap_or(9), row.number));
    Ok(rows)
}

fn collect(
    directory: &Path,
    repo_root: &Path,
    rows: &mut Vec<GateDeclaration>,
    seen: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    let entries = fs::read_dir(directory).map_err(|error| format!("{}: {error}", directory.display()))?;
    let mut paths = entries.map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>().map_err(|error| error.to_string())?;
    paths.sort();
    for path in paths {
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        if path.is_dir() {
            if !matches!(name, "target" | "node_modules" | "tests" | "fixtures" | "gates") {
                collect(&path, repo_root, rows, seen)?;
            }
            continue;
        }
        let extension = path.extension().and_then(|extension| extension.to_str()).unwrap_or_default();
        if !matches!(extension, "rs" | "sh" | "py" | "registry") || name.ends_with("_tests.rs") || name == "tests.rs" {
            continue;
        }
        let source = fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        if !source.contains("gate[") {
            continue;
        }
        let relative = path.strip_prefix(repo_root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
        for (index, line) in source.lines().enumerate() {
            let Some((code, stage, title, policy)) = parse(line)? else { continue };
            let location = format!("{relative}:{}", index + 1);
            if let Some(previous) = seen.insert(code.clone(), location.clone()) {
                return Err(format!("gate code {code} is declared twice: {previous} and {location}"));
            }
            let family = code.chars().next().expect("code has a family");
            let number = code[2..].parse().expect("code has a number");
            rows.push(GateDeclaration { family, number, code, stage, title, source: location, policy });
        }
    }
    Ok(())
}

/// Delivery-stage declarations require an explicit integrity/as-of policy.
fn parse(line: &str) -> Result<Option<(String, String, String, Option<String>)>, String> {
    let trimmed = line.trim_start();
    let Some(body) = ["///", "//!", "//", "#", "<!--", "*"]
        .iter().find_map(|marker| trimmed.strip_prefix(marker))
        .and_then(|body| body.trim_start().strip_prefix("gate[")) else { return Ok(None) };
    let Some((head, title)) = body.split_once("]:") else { return Ok(None) };
    let mut words = head.split_whitespace();
    let (Some(code), Some(stage)) = (words.next(), words.next()) else { return Ok(None) };
    let policy = words.next();
    let Some((family, number)) = code.split_once('-') else { return Ok(None) };
    let valid_code = matches!(family, "G" | "H" | "W" | "N" | "C")
        && (2..=3).contains(&number.len())
        && number.bytes().all(|byte| byte.is_ascii_digit());
    let valid_stage = !stage.is_empty() && stage.bytes().all(|byte| byte.is_ascii_lowercase() || byte == b'-');
    let title = title.trim().trim_end_matches("-->").trim_end_matches("*/").trim();
    if !valid_code || !valid_stage || title.is_empty() { return Ok(None) }
    if words.next().is_some() { return Err(format!("invalid gate declaration: {line}")) }
    let valid_policy = if DELIVERY_STAGES.contains(&stage) {
        matches!(policy, Some("integrity" | "as-of"))
    } else { policy.is_none() };
    if !valid_policy {
        return Err(format!("gate {code} at {stage} requires an explicit delivery policy only at a delivery stage"));
    }
    Ok(Some((code.to_string(), stage.to_string(), title.to_string(), policy.map(str::to_string))))
}
