//! Immutable Git reads shared by a release's member measurements (cas-ac43).
use super::epic_measurement::CommandExt as _;
use std::collections::HashMap;
use std::io::{Seek, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

const LIMIT: usize = 4096;
type Key = (PathBuf, String);

pub(super) fn oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

static COMMITS: OnceLock<Mutex<HashMap<Key, String>>> = OnceLock::new();

/// Movable refs are always resolved again. Only successful immutable-object
/// reads are reused; errors, missing objects and expired budgets stay retryable.
pub(super) fn commit(repo: &Path, reference: &str) -> Option<String> {
    let key = (repo.to_path_buf(), reference.to_string());
    let cache = COMMITS.get_or_init(Default::default);
    if oid(reference) {
        if let Some(value) = cache.lock().ok().and_then(|cache| cache.get(&key).cloned()) { return Some(value); }
    }
    let value = super::resolve_branch_sha(repo, &format!("{reference}^{{commit}}"))?;
    if oid(reference) && let Ok(mut entries) = cache.lock() {
        if entries.len() >= LIMIT { entries.clear(); }
        entries.insert(key, value.clone());
    }
    Some(value)
}

#[derive(Clone)]
pub(super) struct Facts {
    pub(super) parents: Vec<String>,
    pub(super) message: String,
    pub(super) epoch: String,
    pub(super) paths: Vec<String>,
}

static FACTS: OnceLock<Mutex<HashMap<Key, Facts>>> = OnceLock::new();

pub(super) fn facts(repo: &Path, sha: &str) -> Option<Facts> {
    FACTS.get()?.lock().ok()?.get(&(repo.to_path_buf(), sha.to_string())).cloned()
}

/// One metadata read and one first-parent path read replace per-commit
/// `show`, `rev-parse` and `diff` children. Any malformed batch falls back to
/// the existing probes. Raw records consume their path separately, so a
/// filename that looks like an object ID cannot become a record boundary.
pub(super) fn prime(repo: &Path, commits: &[String]) {
    let missing: Vec<_> = commits.iter().filter(|sha| oid(sha) && facts(repo, sha).is_none()).collect();
    if missing.is_empty() { return; }
    let read = || -> Option<Vec<(String, Facts)>> {
        let mut input = tempfile::tempfile().ok()?;
        for sha in &missing { writeln!(input, "{sha}").ok()?; }
        input.rewind().ok()?;
        let output = Command::new("git")
            .args(["log", "--no-walk=unsorted", "--format=%H%x00%P%x00%ct%x00%B%x00", "--stdin"])
            .current_dir(repo).measurement_output_with_stdin(Stdio::from(input)).ok()?;
        if !output.status.success() { return None; }
        let text = String::from_utf8(output.stdout).ok()?;
        let fields: Vec<_> = text.split('\0').collect();
        let mut result = Vec::new();
        for chunk in fields.chunks(4) {
            if chunk.len() == 1 && chunk[0].trim().is_empty() { continue; }
            if chunk.len() != 4 { return None; }
            let sha = chunk[0].trim();
            if !missing.iter().any(|wanted| wanted.as_str() == sha) { return None; }
            let parents: Vec<String> = chunk[1].split_whitespace().map(str::to_string).collect();
            if parents.iter().any(|parent| !oid(parent)) { return None; }
            result.push((sha.to_string(), Facts { parents, epoch: chunk[2].to_string(),
                message: chunk[3].trim().to_string(), paths: Vec::new() }));
        }
        if result.len() != missing.len() { return None; }
        let mut input = tempfile::tempfile().ok()?;
        for (sha, facts) in &result {
            if let Some(parent) = facts.parents.first() { writeln!(input, "{sha} {parent}").ok()?; }
        }
        input.rewind().ok()?;
        let output = Command::new("git")
            .args(["diff-tree", "--stdin", "--raw", "-r", "-z", "--no-renames", "--always"])
            .current_dir(repo).measurement_output_with_stdin(Stdio::from(input)).ok()?;
        if !output.status.success() { return None; }
        let mut fields = output.stdout.split(|byte| *byte == 0).peekable();
        for (sha, facts) in &mut result {
            if facts.parents.is_empty() { continue; }
            if fields.next()? != sha.as_bytes() { return None; }
            while fields.peek().is_some_and(|field| field.starts_with(b":")) {
                fields.next()?;
                facts.paths.push(String::from_utf8(fields.next()?.to_vec()).ok()?);
            }
        }
        if fields.any(|field| !field.is_empty()) { return None; }
        Some(result)
    };
    if let Some(result) = read() {
        if let Ok(mut cache) = FACTS.get_or_init(Default::default).lock() {
            if cache.len() + result.len() > LIMIT { cache.clear(); }
            for (sha, value) in result.into_iter().take(LIMIT) { cache.insert((repo.to_path_buf(), sha), value); }
        }
    }
}

pub(super) fn text(repo: &Path, args: &[&str]) -> Option<String> {
    match args {
        ["show", "-s", format, sha] => {
            if let Some(facts) = facts(repo, sha) {
                match *format {
                    "--format=%B" => return Some(facts.message),
                    "--format=%s" => return Some(facts.message.lines().next().unwrap_or_default().to_string()),
                    "--format=%ct" => return Some(facts.epoch),
                    _ => {}
                }
            }
        }
        ["rev-parse", reference] => {
            if let Some(sha) = reference.strip_suffix("^1") {
                if let Some(parent) = facts(repo, sha).and_then(|facts| facts.parents.first().cloned()) {
                    return Some(parent);
                }
            }
        }
        _ => {}
    }
    crate::git_evidence::git_text(repo, args)
}
