//! Local files for `cas violet`: read from disk, hashed, and encoded the way
//! the hub's own client does, so their bytes never pass through a model.

use std::io::Read;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::hub::VioletError;

/// The hub's inline limit: decoded bytes per call, summed across `files[]`
/// and across every reply of a thread. Larger posts use `file_external`.
pub const INLINE_LIMIT: u64 = 1_048_576;
/// Files one Slack message can carry.
pub const MAX_FILES_PER_MESSAGE: usize = 10;

/// Formats whose bytes can happen to be valid UTF-8 (a small PDF, say) but
/// must still travel as base64.
const BINARY_SIGNATURES: [&[u8]; 6] = [
    b"%PDF-",
    b"\x89PNG\r\n\x1a\n",
    b"\xff\xd8\xff",
    b"PK\x03\x04",
    b"GIF87a",
    b"GIF89a",
];

/// A file that was found, measured and hashed. Only metadata is kept; bytes
/// are read again when an inline payload or an upload needs them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFile {
    pub path: PathBuf,
    pub filename: String,
    pub size_bytes: u64,
    pub sha256: String,
}

impl LocalFile {
    pub fn inspect(path: &Path) -> Result<Self, VioletError> {
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string)
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| {
                VioletError::local(
                    "invalid_input",
                    format!("{} does not name a file", path.display()),
                )
            })?;
        let unreadable = || {
            VioletError::local(
                "local_request_failed",
                format!(
                    "could not read {}; check the path and permissions",
                    path.display()
                ),
            )
        };
        let mut file = std::fs::File::open(path).map_err(|_| unreadable())?;
        if !file.metadata().map_err(|_| unreadable())?.is_file() {
            return Err(VioletError::local(
                "invalid_input",
                format!("{} is not a regular file", path.display()),
            ));
        }
        let mut hasher = Sha256::new();
        let mut size_bytes = 0u64;
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer).map_err(|_| unreadable())?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            size_bytes += read as u64;
        }
        if size_bytes == 0 {
            return Err(VioletError::local(
                "invalid_input",
                format!("{} is empty; Slack refuses an empty file", path.display()),
            ));
        }
        Ok(Self {
            path: path.to_path_buf(),
            filename,
            size_bytes,
            sha256: format!("{:x}", hasher.finalize()),
        })
    }

    /// The bytes, refusing a file that changed after it was hashed.
    fn read_checked(&self) -> Result<Vec<u8>, VioletError> {
        let bytes = std::fs::read(&self.path).map_err(|_| {
            VioletError::local(
                "local_request_failed",
                format!("could not read {}", self.path.display()),
            )
        })?;
        if bytes.len() as u64 != self.size_bytes
            || format!("{:x}", Sha256::digest(&bytes)) != self.sha256
        {
            return Err(VioletError::local(
                "local_request_failed",
                format!(
                    "{} changed while it was being posted; nothing was sent",
                    self.path.display()
                ),
            ));
        }
        Ok(bytes)
    }

    /// An inline `files[]` entry. The declared size and hash let the hub
    /// refuse a corrupted copy before anything reaches Slack.
    pub fn inline_entry(&self) -> Result<Value, VioletError> {
        let bytes = self.read_checked()?;
        let (content, encoding) = encode_content(&bytes);
        Ok(json!({
            "filename": self.filename,
            "content": content,
            "content_encoding": encoding,
            "size_bytes": self.size_bytes,
            "sha256": self.sha256,
        }))
    }

    /// `file_external` metadata: no bytes.
    pub fn external_metadata(&self) -> Value {
        json!({
            "filename": self.filename,
            "size_bytes": self.size_bytes,
            "sha256": self.sha256,
        })
    }
}

/// Text when the bytes are UTF-8 without binary control characters and carry
/// no binary signature; base64 otherwise. Either way the bytes are preserved
/// exactly.
pub fn encode_content(bytes: &[u8]) -> (String, &'static str) {
    if !BINARY_SIGNATURES
        .iter()
        .any(|signature| bytes.starts_with(signature))
        && let Ok(text) = std::str::from_utf8(bytes)
        && !text.chars().any(|c| {
            (c < ' ' && !matches!(c, '\t' | '\r' | '\n')) || ('\u{7f}'..='\u{9f}').contains(&c)
        })
    {
        return (text.to_string(), "text");
    }
    (
        base64::engine::general_purpose::STANDARD.encode(bytes),
        "base64",
    )
}

/// How a set of files reaches Slack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// No files: a plain message.
    Message,
    /// Every file inside one call, at most [`INLINE_LIMIT`] bytes in total.
    Inline,
    /// `file_external`: begin, direct upload from disk, complete.
    External,
}

impl Route {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Inline => "inline",
            Self::External => "file_external",
        }
    }
}

pub fn choose_route<'a>(files: impl IntoIterator<Item = &'a LocalFile>) -> Route {
    let mut any = false;
    let mut total = 0u64;
    for file in files {
        any = true;
        total = total.saturating_add(file.size_bytes);
    }
    match (any, total <= INLINE_LIMIT) {
        (false, _) => Route::Message,
        (true, true) => Route::Inline,
        (true, false) => Route::External,
    }
}

pub fn inspect_all(paths: &[PathBuf]) -> Result<Vec<LocalFile>, VioletError> {
    paths.iter().map(|path| LocalFile::inspect(path)).collect()
}
