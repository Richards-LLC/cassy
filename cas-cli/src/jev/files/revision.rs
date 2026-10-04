//! Git snapshot reads without checkout, filters, or symlink traversal.
use super::{JevError, MAX_FILE_BYTES, secret};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

struct Entry {
    mode: String,
    oid: String,
}
pub(super) struct Revision {
    root: PathBuf,
    pub commit: String,
    entries: BTreeMap<PathBuf, Entry>,
    ignores: Vec<(PathBuf, ignore::gitignore::Gitignore)>,
}
impl Revision {
    pub fn open(root: &Path, rev: &str) -> Result<Self, JevError> {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args([
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{rev}^{{commit}}"),
            ])
            .output()
            .map_err(|_| JevError::InvalidInput("Cannot resolve Git revision".into()))?;
        let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !output.status.success()
            || !matches!(commit.len(), 40 | 64)
            || !commit.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(JevError::InvalidInput(
                "Missing or invalid Git revision".into(),
            ));
        }
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["ls-tree", "-r", "-z", &commit])
            .output()
            .map_err(|_| JevError::InvalidInput("Cannot list Git revision".into()))?;
        if !output.status.success() {
            return Err(JevError::InvalidInput("Cannot list Git revision".into()));
        }
        let mut entries = BTreeMap::new();
        for row in output
            .stdout
            .split(|b| *b == 0)
            .filter(|row| !row.is_empty())
        {
            let Some(tab) = row.iter().position(|b| *b == b'\t') else {
                continue;
            };
            let (Ok(meta), Ok(path)) = (
                std::str::from_utf8(&row[..tab]),
                std::str::from_utf8(&row[tab + 1..]),
            ) else {
                continue;
            };
            let parts: Vec<_> = meta.split_whitespace().collect();
            if parts.len() == 3 {
                entries.insert(
                    PathBuf::from(path),
                    Entry {
                        mode: parts[0].into(),
                        oid: parts[2].into(),
                    },
                );
            }
        }
        let mut snapshot = Self {
            root: root.into(),
            commit,
            entries,
            ignores: vec![],
        };
        // Ignore policy comes from the same commit, never from a dirty checkout.
        let ignore_paths: Vec<_> = snapshot
            .entries
            .keys()
            .filter(|p| {
                !secret(p)
                    && matches!(
                        p.file_name().and_then(|n| n.to_str()),
                        Some(".gitignore" | ".ignore")
                    )
            })
            .cloned()
            .collect();
        for path in ignore_paths {
            let bytes = snapshot
                .read(&path, MAX_FILE_BYTES)
                .map_err(JevError::InvalidInput)?;
            if bytes.len() > MAX_FILE_BYTES {
                return Err(JevError::InvalidInput(
                    "Revision ignore file exceeds byte cap".into(),
                ));
            }
            let text = std::str::from_utf8(&bytes)
                .map_err(|_| JevError::InvalidInput("Revision ignore file is not UTF-8".into()))?;
            let dir = snapshot.root.join(path.parent().unwrap_or(Path::new("")));
            let mut builder = ignore::gitignore::GitignoreBuilder::new(&dir);
            for line in text.lines() {
                builder
                    .add_line(Some(path.clone()), line)
                    .map_err(|_| JevError::InvalidInput("Invalid revision ignore rule".into()))?;
            }
            snapshot.ignores.push((
                dir,
                builder
                    .build()
                    .map_err(|_| JevError::InvalidInput("Invalid revision ignore rules".into()))?,
            ));
        }
        snapshot
            .ignores
            .sort_by_key(|(dir, _)| dir.components().count());
        Ok(snapshot)
    }
    pub fn contains(&self, path: &Path) -> bool {
        self.entries.contains_key(path)
    }
    pub fn is_directory(&self, path: &Path) -> bool {
        self.entries
            .keys()
            .any(|p| p != path && p.starts_with(path))
    }
    pub fn paths(&self) -> Vec<PathBuf> {
        self.entries
            .keys()
            .filter(|path| {
                let absolute = self.root.join(path);
                // A child negation cannot reinclude a file beneath an ignored parent.
                for ancestor in absolute.ancestors().take_while(|p| *p != self.root) {
                    let mut ignored = false;
                    for (dir, rules) in &self.ignores {
                        if ancestor.starts_with(dir) {
                            let matched = rules.matched(ancestor, ancestor != absolute);
                            if !matched.is_none() {
                                ignored = matched.is_ignore();
                            }
                        }
                    }
                    if ignored {
                        return false;
                    }
                }
                !path
                    .components()
                    .any(|part| matches!(part.as_os_str().to_str(), Some(".git" | ".cas")))
            })
            .cloned()
            .collect()
    }
    pub fn read(&self, path: &Path, cap: usize) -> Result<Vec<u8>, String> {
        let entry = self.entries.get(path).ok_or("missing at revision")?;
        match entry.mode.as_str() {
            "100644" | "100755" => {}
            "120000" => return Err("symlink file (not followed)".into()),
            _ => return Err("non-regular file".into()),
        }
        let mut child = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(["cat-file", "blob", &entry.oid])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "unreadable revision blob")?;
        let mut bytes = Vec::new();
        let read = child
            .stdout
            .take()
            .ok_or("unreadable revision blob")?
            .take(cap as u64 + 1)
            .read_to_end(&mut bytes);
        // Stop decompression after the cap, and always reap the child.
        if bytes.len() > cap || read.is_err() {
            let _ = child.kill();
        }
        let status = child.wait().map_err(|_| "unreadable revision blob")?;
        if read.is_err() || (bytes.len() <= cap && !status.success()) {
            return Err("unreadable revision blob".into());
        }
        Ok(bytes)
    }
}
