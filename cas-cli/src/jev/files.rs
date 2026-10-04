//! Bounded file questions. Contents enter Jev state, never the returned rows.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Component;

pub const DEFAULT_FILE_BYTES: usize = 24 * 1024;
pub const MAX_FILE_BYTES: usize = 128 * 1024;
mod revision;
use revision::Revision;

#[derive(Debug, Clone)]
pub struct FilesOptions {
    pub paths: Vec<String>,
    pub globs: Vec<String>,
    pub recursive: bool,
    pub max_files: usize,
    pub max_bytes: usize,
    pub offset: usize,
    pub rev: Option<String>,
}
impl Default for FilesOptions {
    fn default() -> Self {
        Self {
            paths: vec![],
            globs: vec![],
            recursive: false,
            max_files: 50,
            max_bytes: DEFAULT_FILE_BYTES,
            offset: 0,
            rev: None,
        }
    }
}
#[derive(Debug, Serialize)]
pub struct FilesResponse {
    pub files: Vec<FileRow>,
    /// More matching candidates existed than the selection cap, or scanning timed out.
    pub limit_reached: bool,
    pub next_offset: Option<usize>,
    /// Resolved immutable commit when rev was supplied.
    pub revision: Option<String>,
}
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum FileRow {
    Available {
        path: String,
        truncated: bool,
        model: String,
        answers: BTreeMap<String, Answer>,
        usage: Usage,
    },
    Incomplete {
        path: String,
        truncated: bool,
        reason: String,
    },
    Unavailable {
        path: String,
        reason: String,
    },
    Skipped {
        path: String,
        reason: String,
    },
}
fn skipped(path: impl Into<String>, reason: &str) -> FileRow {
    FileRow::Skipped {
        path: path.into(),
        reason: reason.into(),
    }
}

// Normalize without touching the filesystem; glob metacharacters remain literal.
fn relative(root: &Path, supplied_root: &Path, supplied: &str) -> Option<PathBuf> {
    let input = Path::new(supplied);
    // Preserve lexical secret components while accepting the supplied root
    // through an OS alias (for example macOS /var -> /private/var).
    let input = if input.is_absolute() {
        input.strip_prefix(supplied_root).unwrap_or(input)
    } else {
        input
    };
    let mut path = if input.is_absolute() {
        PathBuf::new()
    } else {
        root.to_path_buf()
    };
    for component in input.components() {
        match component {
            Component::ParentDir => {
                if !path.pop() {
                    return None;
                }
            }
            Component::CurDir => {}
            other => path.push(other.as_os_str()),
        }
    }
    path.strip_prefix(root).ok().map(Path::to_path_buf)
}
fn secret(path: &Path) -> bool {
    path.components().any(|part| {
        let name = part.as_os_str().to_string_lossy().to_ascii_lowercase();
        name.starts_with(".env")
            || name.ends_with(".pem")
            || matches!(
                name.as_str(),
                "creds"
                    | "credentials"
                    | ".credentials"
                    | "secrets"
                    | ".secrets"
                    | ".ssh"
                    | ".aws"
                    | ".gnupg"
                    | ".git"
                    | ".cas"
            )
    })
}
fn label(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

impl JevClient {
    pub fn files(
        &self,
        project_root: &Path,
        options: &FilesOptions,
        questions: &Value,
        caller: &str,
        advisory: bool,
    ) -> Result<FilesResponse, JevError> {
        validate_input(&json!({}), questions)?;
        if !(1..=50).contains(&options.max_files)
            || !(1..=MAX_FILE_BYTES).contains(&options.max_bytes)
        {
            return Err(JevError::InvalidInput(
                "Jev files requires max_files 1–50 and max_bytes 1–131072".into(),
            ));
        }
        if options.paths.is_empty() && options.globs.is_empty() {
            return Err(JevError::InvalidInput(
                "Jev files requires paths or globs".into(),
            ));
        }
        let root = project_root
            .canonicalize()
            .map_err(|_| JevError::InvalidInput("Cannot resolve Jev project root".into()))?;
        let deadline = Instant::now() + BATCH_TIMEOUT;
        let revision = options
            .rev
            .as_deref()
            .map(|rev| Revision::open(&root, rev))
            .transpose()?;
        let mut output = FilesResponse {
            files: vec![],
            limit_reached: false,
            next_offset: None,
            revision: revision.as_ref().map(|r| r.commit.clone()),
        };
        let mut exact = BTreeSet::new();
        let mut directories = Vec::new();
        for supplied in &options.paths {
            let Some(rel) = relative(&root, project_root, supplied) else {
                output.files.push(skipped(supplied, "outside project root"));
                continue;
            };
            if secret(&rel) {
                output.files.push(skipped(label(&rel), "secret path"));
                continue;
            }
            if let Some(revision) = &revision {
                if revision.is_directory(&rel) {
                    directories.push(rel);
                } else if revision.contains(&rel) {
                    exact.insert(rel);
                } else {
                    output
                        .files
                        .push(skipped(label(&rel), "missing at revision"));
                }
                continue;
            }
            let path = root.join(&rel);
            let resolved = match path.canonicalize() {
                Ok(path) => path,
                Err(error) => {
                    output.files.push(skipped(
                        label(&rel),
                        if error.kind() == std::io::ErrorKind::NotFound {
                            "missing path"
                        } else {
                            "unreadable path"
                        },
                    ));
                    continue;
                }
            };
            if !resolved.starts_with(&root) {
                output
                    .files
                    .push(skipped(label(&rel), "outside project root"));
                continue;
            }
            if secret(&rel) || secret(resolved.strip_prefix(&root).expect("contained path")) {
                output.files.push(skipped(label(&rel), "secret path"));
                continue;
            }
            if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
                output
                    .files
                    .push(skipped(label(&rel), "symlink file (not followed)"));
                continue;
            }
            if resolved.is_dir() {
                directories.push(rel);
            } else {
                exact.insert(rel);
            }
        }
        let mut patterns = Vec::new();
        for supplied in &options.globs {
            let Some(rel) = relative(&root, project_root, supplied) else {
                output.files.push(skipped(supplied, "outside project root"));
                continue;
            };
            patterns.push(
                glob::Pattern::new(&label(&rel))
                    .map_err(|_| JevError::InvalidInput("Invalid Jev file glob".into()))?,
            );
        }
        if exact.is_empty() && directories.is_empty() && patterns.is_empty() {
            return Ok(output);
        }
        let mut found = BTreeSet::new();
        let mut selected = BTreeSet::new();
        let matches = glob::MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: false,
        };
        // Root walking applies ignore rules even to explicitly supplied files.
        // Hidden files are considered so secret refusals cannot depend on dotfile hiding.
        let candidates: Box<dyn Iterator<Item = PathBuf>> = if let Some(revision) = &revision {
            Box::new(revision.paths().into_iter())
        } else {
            Box::new(
                ignore::WalkBuilder::new(&root)
                    .hidden(false)
                    .require_git(false)
                    .git_global(false)
                    .follow_links(false)
                    .sort_by_file_name(|a, b| a.cmp(b))
                    .filter_entry(|entry| {
                        !matches!(entry.file_name().to_str(), Some(".git" | ".cas"))
                    })
                    .build()
                    .filter_map(|entry| {
                        let entry = entry.ok()?;
                        if !entry
                            .file_type()
                            .is_some_and(|ft| ft.is_file() || ft.is_symlink())
                        {
                            return None;
                        }
                        entry.path().strip_prefix(&root).ok().map(Path::to_path_buf)
                    }),
            )
        };
        let mut matched = 0usize;
        for rel in candidates {
            if Instant::now() >= deadline {
                output.limit_reached = true;
                break;
            }
            let rel = rel.as_path();
            if !(exact.contains(rel)
                || directories.iter().any(|dir| {
                    rel.strip_prefix(dir)
                        .is_ok_and(|tail| options.recursive || tail.components().count() == 1)
                })
                || patterns
                    .iter()
                    .any(|p| p.matches_with(&label(rel), matches)))
            {
                continue;
            }
            found.insert(rel.to_path_buf());
            if matched < options.offset {
                matched += 1;
                continue;
            }
            if selected.len() == options.max_files {
                output.limit_reached = true;
                break;
            }
            selected.insert(rel.to_path_buf());
        }
        if output.limit_reached {
            output.next_offset = Some(options.offset.saturating_add(selected.len()));
        }
        for rel in exact.difference(&found).filter(|_| !output.limit_reached) {
            output
                .files
                .push(skipped(label(rel), "ignored by file selection rules"));
        }
        let mut first_error = None;
        for rel in selected {
            let path = label(&rel);
            let mut bytes = if let Some(revision) = &revision {
                if secret(&rel) {
                    output.files.push(skipped(path, "secret path"));
                    continue;
                }
                match revision.read(&rel, options.max_bytes) {
                    Ok(bytes) => bytes,
                    Err(reason) => {
                        output.files.push(skipped(path, &reason));
                        continue;
                    }
                }
            } else {
                let resolved = match root.join(&rel).canonicalize() {
                    Ok(p) if p.starts_with(&root) => p,
                    Ok(_) => {
                        output.files.push(skipped(path, "outside project root"));
                        continue;
                    }
                    Err(_) => {
                        output
                            .files
                            .push(skipped(path, "missing or unreadable file"));
                        continue;
                    }
                };
                if secret(&rel) || secret(resolved.strip_prefix(&root).expect("contained path")) {
                    output.files.push(skipped(path, "secret path"));
                    continue;
                }
                if fs::symlink_metadata(root.join(&rel)).is_ok_and(|m| m.file_type().is_symlink()) {
                    output
                        .files
                        .push(skipped(path, "symlink file (not followed)"));
                    continue;
                }
                if !fs::metadata(&resolved).is_ok_and(|m| m.is_file()) {
                    output.files.push(skipped(path, "non-regular file"));
                    continue;
                }
                let file = match fs::File::open(&resolved) {
                    Ok(file) if file.metadata().is_ok_and(|m| m.is_file()) => file,
                    _ => {
                        output
                            .files
                            .push(skipped(path, "unreadable or non-regular file"));
                        continue;
                    }
                };
                let mut bytes = Vec::new();
                if file
                    .take(options.max_bytes as u64 + 1)
                    .read_to_end(&mut bytes)
                    .is_err()
                {
                    output.files.push(skipped(path, "unreadable file"));
                    continue;
                }
                bytes
            };
            let truncated = bytes.len() > options.max_bytes;
            bytes.truncate(options.max_bytes);
            if truncated {
                output.files.push(FileRow::Incomplete {
                    path, truncated: true,
                    reason: "File exceeds max_bytes; no answers returned because a truncated prefix cannot prove absence. Increase max_bytes or narrow the input.".into(),
                });
                continue;
            }
            if bytes.contains(&0) {
                output.files.push(skipped(path, "binary file"));
                continue;
            }
            let content = match std::str::from_utf8(&bytes) {
                Ok(s) => s.to_string(),
                Err(_) => {
                    output.files.push(skipped(path, "binary or non-UTF-8 file"));
                    continue;
                }
            };
            let mut state = json!({"path":path,"content":content});
            if let Some(revision) = &output.revision {
                state["revision"] = json!(revision);
            }
            match self.ask_until(
                &state,
                questions,
                caller,
                advisory,
                deadline.min(Instant::now() + self.timeout),
            ) {
                Ok(Outcome::Available(response)) => output.files.push(FileRow::Available {
                    path,
                    truncated,
                    model: response.model,
                    answers: response.answers,
                    usage: response.usage,
                }),
                Ok(Outcome::Unavailable { reason, .. }) => {
                    output.files.push(FileRow::Unavailable { path, reason })
                }
                Err(e) => {
                    if first_error.is_none() {
                        first_error = Some(e);
                    }
                }
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(output)
    }
}
