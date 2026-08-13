//! Discovery and normalization of durable factory artifacts for search.

use std::path::{Path, PathBuf};

/// Artifacts larger than this are delivery evidence, but not search input.
pub const MAX_ARTIFACT_BYTES: u64 = 1_048_576;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactDocument {
    pub task_id: String,
    pub path: PathBuf,
    pub content: String,
}

/// Stable index identifier for one artifact path owned by one task.
pub fn artifact_document_id(task_id: &str, path: &Path) -> String {
    format!("artifact::{task_id}::{}", path.display())
}

/// Decode an artifact index identifier for user-facing search rendering.
pub fn parse_artifact_document_id(id: &str) -> Option<(&str, &str)> {
    let remainder = id.strip_prefix("artifact::")?;
    remainder.split_once("::")
}

/// Find supported text artifacts beneath every direct task directory.
pub fn discover_all_task_artifacts(artifacts_root: &Path) -> Vec<ArtifactDocument> {
    let Ok(entries) = std::fs::read_dir(artifacts_root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            entry
                .file_type()
                .ok()
                .filter(|file_type| file_type.is_dir())
                .and_then(|_| entry.file_name().into_string().ok())
        })
        .flat_map(|task_id| discover_task_artifacts(artifacts_root, &task_id))
        .collect()
}

/// Find supported text artifacts beneath one task's durable directory.
pub fn discover_task_artifacts(artifacts_root: &Path, task_id: &str) -> Vec<ArtifactDocument> {
    let task_root = artifacts_root.join(task_id);
    let mut paths = vec![task_root];
    let mut artifacts = Vec::new();

    while let Some(path) = paths.pop() {
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&path) {
                paths.extend(entries.flatten().map(|entry| entry.path()));
            }
            continue;
        }
        if !metadata.is_file() || metadata.len() > MAX_ARTIFACT_BYTES || !is_text_artifact(&path) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        // NUL bytes are a cheap, reliable binary signal even in otherwise
        // valid UTF-8. Invalid UTF-8 is likewise not a text artifact.
        if bytes.contains(&0) {
            continue;
        }
        let Ok(content) = String::from_utf8(bytes) else {
            continue;
        };
        artifacts.push(ArtifactDocument {
            task_id: task_id.to_string(),
            path,
            content,
        });
    }

    artifacts
}

fn is_text_artifact(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "md" | "txt" | "json"
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_skips_binary_oversize_and_unsupported_artifacts() {
        let temp = tempfile::tempdir().unwrap();
        let task_root = temp.path().join("cas-artifacts");
        std::fs::create_dir_all(&task_root).unwrap();
        std::fs::write(task_root.join("SEND-LOG.md"), "MessageId: readable").unwrap();
        std::fs::write(task_root.join("binary.txt"), b"\0not text").unwrap();
        std::fs::write(task_root.join("image.png"), "not indexed").unwrap();

        let documents = discover_task_artifacts(temp.path(), "cas-artifacts");
        assert_eq!(documents.len(), 1);
        assert_eq!(documents[0].path, task_root.join("SEND-LOG.md"));
    }
}
