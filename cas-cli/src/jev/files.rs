//! Bounded file questions. Contents enter Jev state, never the returned rows.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Component;

pub const DEFAULT_FILE_BYTES: usize = 24 * 1024;
pub const MAX_FILE_BYTES: usize = 128 * 1024;
const TRUNCATION_MARKER: &str = "\n[Jev: file truncated at byte cap]";

#[derive(Debug, Clone)]
pub struct FilesOptions {
    pub paths: Vec<String>,
    pub globs: Vec<String>,
    pub recursive: bool,
    pub max_files: usize,
    pub max_bytes: usize,
}
impl Default for FilesOptions {
    fn default() -> Self {
        Self {
            paths: vec![],
            globs: vec![],
            recursive: false,
            max_files: 50,
            max_bytes: DEFAULT_FILE_BYTES,
        }
    }
}
#[derive(Debug, Serialize)]
pub struct FilesResponse {
    pub files: Vec<FileRow>,
    /// More matching candidates existed than the selection cap, or scanning timed out.
    pub limit_reached: bool,
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
        let mut output = FilesResponse {
            files: vec![],
            limit_reached: false,
        };
        let mut exact = BTreeSet::new();
        let mut directories = Vec::new();
        for supplied in &options.paths {
            let Some(rel) = relative(&root, project_root, supplied) else {
                output.files.push(skipped(supplied, "outside project root"));
                continue;
            };
            let path = root.join(&rel);
            let resolved = match path.canonicalize() {
                Ok(path) => path,
                Err(_) => {
                    output
                        .files
                        .push(skipped(label(&rel), "missing or unreadable path"));
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
        let walker = ignore::WalkBuilder::new(&root)
            .hidden(false)
            .require_git(false)
            .git_global(false)
            .follow_links(false)
            .sort_by_file_name(|a, b| a.cmp(b))
            .filter_entry(|entry| !matches!(entry.file_name().to_str(), Some(".git" | ".cas")))
            .build();
        for entry in walker {
            if Instant::now() >= deadline {
                output.limit_reached = true;
                break;
            }
            let Ok(entry) = entry else {
                continue;
            };
            if !entry
                .file_type()
                .is_some_and(|ft| ft.is_file() || ft.is_symlink())
            {
                continue;
            }
            let Ok(rel) = entry.path().strip_prefix(&root) else {
                continue;
            };
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
            if selected.len() == options.max_files {
                output.limit_reached = true;
                break;
            }
            selected.insert(rel.to_path_buf());
        }
        for rel in exact.difference(&found) {
            output.files.push(skipped(
                label(rel),
                if output.limit_reached {
                    "selection limit reached"
                } else {
                    "ignored by file selection rules"
                },
            ));
        }
        let mut first_error = None;
        for rel in selected {
            let path = label(&rel);
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
            let truncated = bytes.len() > options.max_bytes;
            bytes.truncate(options.max_bytes);
            if bytes.contains(&0) {
                output.files.push(skipped(path, "binary file"));
                continue;
            }
            // Only an incomplete UTF-8 codepoint at a truncated boundary may be trimmed.
            let content = match std::str::from_utf8(&bytes) {
                Ok(s) => s.to_string(),
                Err(e) if truncated && e.error_len().is_none() => {
                    String::from_utf8(bytes[..e.valid_up_to()].to_vec()).expect("valid prefix")
                }
                Err(_) => {
                    output.files.push(skipped(path, "binary or non-UTF-8 file"));
                    continue;
                }
            };
            let content = if truncated {
                format!("{content}{TRUNCATION_MARKER}")
            } else {
                content
            };
            match self.ask_until(
                &json!({"path":path,"content":content}),
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
