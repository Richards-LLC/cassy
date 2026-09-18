//! Published artifacts: the durable reference one lane mints and another renders.
//!
//! A worker publishes a local file (a PDF report, a capture, a log bundle) and
//! gets back an [`ArtifactRef`]. That ref is what travels — into a typed
//! operator message, a task note, a Slack card. It deliberately carries no
//! URL and no upload credential: those live on the `artifacts` row, change
//! after the ref is minted, and a persisted upload URL is a credential leak.
//! A consumer holding a ref asks the runtime for a fresh location.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use crate::TypeError;

/// Hard ceiling on a publishable artifact, enforced before any network call.
pub const ARTIFACT_MAX_BYTES: u64 = 25 * 1024 * 1024;

/// The portable half of a published artifact.
///
/// Every field is verifiable by the receiver: `size_bytes` and `sha256` are
/// computed from the bytes on disk before upload, so a consumer that later
/// downloads the artifact can prove it received what was published.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRef {
    /// Local record id, e.g. `art-7f3a9c21`.
    pub artifact_id: String,
    /// File name as published, e.g. `2026-09-18-release-brief.pdf`.
    pub name: String,
    /// Media type derived from the extension, e.g. `application/pdf`.
    pub mime: String,
    pub size_bytes: u64,
    /// Lowercase hex, 64 characters.
    pub sha256: String,
}

impl ArtifactRef {
    /// Reject a ref whose self-describing fields cannot be true, so a
    /// malformed ref fails at its boundary instead of at render time.
    pub fn validate(&self) -> Result<(), TypeError> {
        if self.artifact_id.trim().is_empty() {
            return Err(TypeError::InvalidArtifact(
                "artifact reference requires an artifact_id".to_string(),
            ));
        }
        if self.name.trim().is_empty() {
            return Err(TypeError::InvalidArtifact(
                "artifact reference requires a name".to_string(),
            ));
        }
        if self.name.contains('/') || self.name.contains('\\') {
            return Err(TypeError::InvalidArtifact(format!(
                "artifact name must be a bare file name, got {}",
                self.name
            )));
        }
        if self.size_bytes == 0 {
            return Err(TypeError::InvalidArtifact(
                "artifact reference requires a non-zero size".to_string(),
            ));
        }
        if self.size_bytes > ARTIFACT_MAX_BYTES {
            return Err(TypeError::InvalidArtifact(format!(
                "artifact is {} bytes; the published ceiling is {ARTIFACT_MAX_BYTES}",
                self.size_bytes
            )));
        }
        if self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(TypeError::InvalidArtifact(
                "artifact sha256 must be 64 lowercase hex characters".to_string(),
            ));
        }
        Ok(())
    }

    /// Size rendered for a person: `812 KB`, `3.4 MB`. Decimal units, because
    /// that is what the operating system's file dialog showed them.
    pub fn human_size(&self) -> String {
        const KB: u64 = 1_000;
        const MB: u64 = 1_000_000;
        match self.size_bytes {
            bytes if bytes < KB => format!("{bytes} B"),
            bytes if bytes < MB => format!("{} KB", bytes.div_ceil(KB)),
            bytes => format!("{:.1} MB", bytes as f64 / MB as f64),
        }
    }
}

/// Where a published artifact has got to.
///
/// The states are ordered by how much of the publish pipeline has completed;
/// a row never moves backwards. `Local` is a terminal-for-now state when
/// Cloud storage is unreachable, not an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactStatus {
    /// Hashed and recorded locally; never sent, or Cloud declined it.
    Local,
    /// Bytes reached the upload URL; the server has not confirmed them.
    Uploaded,
    /// The server confirmed size and digest and owns the object.
    Committed,
}

impl ArtifactStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ArtifactStatus::Local => "local",
            ArtifactStatus::Uploaded => "uploaded",
            ArtifactStatus::Committed => "committed",
        }
    }
}

impl fmt::Display for ArtifactStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ArtifactStatus {
    type Err = TypeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "local" => Ok(ArtifactStatus::Local),
            "uploaded" => Ok(ArtifactStatus::Uploaded),
            "committed" => Ok(ArtifactStatus::Committed),
            other => Err(TypeError::InvalidArtifact(format!(
                "unknown artifact status: {other}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid() -> ArtifactRef {
        ArtifactRef {
            artifact_id: "art-7f3a9c21".to_string(),
            name: "2026-09-18-release-brief.pdf".to_string(),
            mime: "application/pdf".to_string(),
            size_bytes: 812_345,
            sha256: "a".repeat(64),
        }
    }

    #[test]
    fn a_well_formed_reference_validates() {
        valid().validate().unwrap();
    }

    #[test]
    fn a_name_carrying_a_path_separator_is_refused() {
        let mut artifact = valid();
        artifact.name = "reports/brief.pdf".to_string();
        let error = artifact.validate().unwrap_err().to_string();
        assert!(error.contains("bare file name"), "{error}");

        artifact.name = r"reports\brief.pdf".to_string();
        assert!(artifact.validate().is_err(), "a backslash escapes too");
    }

    #[test]
    fn a_digest_that_is_not_lowercase_hex_is_refused() {
        let mut artifact = valid();
        artifact.sha256 = "A".repeat(64);
        assert!(
            artifact.validate().is_err(),
            "uppercase hex is a different string and would not compare equal"
        );

        artifact.sha256 = "a".repeat(63);
        assert!(artifact.validate().is_err(), "a short digest is refused");

        artifact.sha256 = "z".repeat(64);
        assert!(artifact.validate().is_err(), "non-hex is refused");
    }

    #[test]
    fn the_size_ceiling_is_enforced_on_the_reference_too() {
        let mut artifact = valid();
        artifact.size_bytes = ARTIFACT_MAX_BYTES;
        artifact.validate().expect("exactly at the ceiling is fine");

        artifact.size_bytes = ARTIFACT_MAX_BYTES + 1;
        let error = artifact.validate().unwrap_err().to_string();
        assert!(error.contains("ceiling"), "{error}");

        artifact.size_bytes = 0;
        assert!(
            artifact.validate().is_err(),
            "an empty artifact is a publish mistake, not a payload"
        );
    }

    #[test]
    fn sizes_read_the_way_a_file_dialog_reads() {
        let mut artifact = valid();
        artifact.size_bytes = 812;
        assert_eq!(artifact.human_size(), "812 B");
        artifact.size_bytes = 812_345;
        assert_eq!(artifact.human_size(), "813 KB");
        artifact.size_bytes = 3_400_000;
        assert_eq!(artifact.human_size(), "3.4 MB");
    }

    #[test]
    fn status_round_trips_through_its_wire_form() {
        for status in [
            ArtifactStatus::Local,
            ArtifactStatus::Uploaded,
            ArtifactStatus::Committed,
        ] {
            assert_eq!(status.as_str().parse::<ArtifactStatus>().unwrap(), status);
            assert_eq!(
                serde_json::to_value(status).unwrap(),
                serde_json::Value::String(status.as_str().to_string()),
                "the JSON form must match the column value"
            );
        }
        assert!("shipped".parse::<ArtifactStatus>().is_err());
    }

    #[test]
    fn the_reference_serializes_in_snake_case() {
        let json = serde_json::to_value(valid()).unwrap();
        for field in ["artifact_id", "name", "mime", "size_bytes", "sha256"] {
            assert!(json.get(field).is_some(), "missing {field} in {json}");
        }
        assert!(
            json.get("cloud_url").is_none() && json.get("status").is_none(),
            "a reference must not carry mutable or credential-bearing state: {json}"
        );
    }
}
