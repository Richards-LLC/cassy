//! Images a supervisor reply names by path (cas-85bfb): only a PNG, JPEG,
//! WebP or GIF file inside the session project's own artifacts directory.
//!
//! The browser sends the path as written, base64url-encoded after
//! [`LOCAL_IMAGE_PREFIX`], through the existing session media ticket and
//! content routes. This module is the only place it becomes a file, and it
//! is not a file-read primitive: the path must lie under
//! `[factory] artifacts_root/<project-key>/`, every component is opened with
//! O_NOFOLLOW beneath that directory (no `..`, no symlinks, no re-resolution
//! race), and the bytes themselves must be one of four image formats whose
//! extension agrees, within a size cap.
use super::composer_upload::open_beneath;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use cas_store::PublishedArtifact;
use std::fs::File;
use std::io::{Read, Seek};
use std::path::{Component, Path, PathBuf};

/// Artifact references starting with this name a local image path.
pub(crate) const LOCAL_IMAGE_PREFIX: &str = "local-";
/// An inline reply image is a screenshot, not a download.
pub(crate) const LOCAL_IMAGE_MAX_BYTES: u64 = 10 * 1024 * 1024;

/// The format the leading bytes prove, and the extensions that may name it.
fn sniff(head: &[u8]) -> Option<(&'static str, &'static [&'static str])> {
    if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(("image/png", &["png"]))
    } else if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(("image/jpeg", &["jpg", "jpeg"]))
    } else if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        Some(("image/gif", &["gif"]))
    } else if head.len() >= 12 && head.starts_with(b"RIFF") && &head[8..12] == b"WEBP" {
        Some(("image/webp", &["webp"]))
    } else {
        None
    }
}

/// Resolve a `local-…` reference for the project whose `.cas` is `cas_root`.
/// Every refusal is the same opaque error; the route answers a generic 404.
pub(crate) fn open_local_image(cas_root: &Path, reference: &str) -> anyhow::Result<(PublishedArtifact, File)> {
    let encoded = reference.strip_prefix(LOCAL_IMAGE_PREFIX).filter(|value| !value.is_empty() && value.len() <= 4_096);
    let encoded = encoded.ok_or_else(|| anyhow::anyhow!("image unavailable"))?;
    let written = String::from_utf8(URL_SAFE_NO_PAD.decode(encoded)?)?;
    let config = crate::config::Config::load(cas_root)?;
    let configured = crate::config::resolved_factory_artifacts_root(config.factory().artifacts_root.as_deref());
    let base = configured.canonicalize()?;
    let project = crate::config::project_factory_artifacts_root(cas_root, &base);
    let written = Path::new(written.trim());
    // An absolute path must name this project's artifacts directory, as
    // configured or canonical; a relative one is read inside it.
    let relative: PathBuf = if written.is_absolute() {
        written
            .strip_prefix(&project)
            .or_else(|_| written.strip_prefix(crate::config::project_factory_artifacts_root(cas_root, &configured)))
            .map_err(|_| anyhow::anyhow!("image unavailable"))?
            .to_path_buf()
    } else {
        written.to_path_buf()
    };
    anyhow::ensure!(
        relative.components().count() > 0 && relative.components().all(|part| matches!(part, Component::Normal(_))),
        "image unavailable"
    );
    let mut file = open_beneath(&base, &project.strip_prefix(&base)?.join(&relative))?;
    let metadata = file.metadata()?;
    anyhow::ensure!(
        metadata.is_file() && metadata.len() > 0 && metadata.len() <= LOCAL_IMAGE_MAX_BYTES,
        "image unavailable"
    );
    let mut head = [0u8; 12];
    let read = file.read(&mut head)?;
    let (mime, extensions) = sniff(&head[..read]).ok_or_else(|| anyhow::anyhow!("image unavailable"))?;
    let extension = relative.extension().and_then(|value| value.to_str()).map(str::to_ascii_lowercase);
    anyhow::ensure!(extension.is_some_and(|value| extensions.contains(&value.as_str())), "image unavailable");
    file.rewind()?;
    let name = relative.file_name().and_then(|value| value.to_str()).unwrap_or("image").to_owned();
    Ok((
        PublishedArtifact {
            id: reference.to_owned(),
            task_id: String::new(),
            name,
            mime: mime.to_owned(),
            size_bytes: metadata.len(),
            sha256: String::new(),
            cloud_artifact_id: None,
            status: "local".to_owned(),
            cloud_url: None,
            slack_permalink: None,
            created_at: String::new(),
            revocation: None,
        },
        file,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_only_real_images_inside_the_project_artifacts_directory() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project/.cas");
        std::fs::create_dir_all(&root).unwrap();
        let artifacts = temp.path().join("artifacts");
        std::fs::write(root.join("config.toml"), format!("[factory]\nartifacts_root = {:?}\n", artifacts.to_str().unwrap())).unwrap();
        std::fs::create_dir_all(&artifacts).unwrap();
        let project = crate::config::project_factory_artifacts_root(&root, &artifacts.canonicalize().unwrap());
        let png = b"\x89PNG\r\n\x1a\n0000";
        std::fs::create_dir_all(project.join("cas-1/after")).unwrap();
        std::fs::write(project.join("cas-1/after/shot.png"), png).unwrap();
        std::fs::write(project.join("cas-1/page.png"), b"<svg onload=alert(1)>").unwrap();
        std::fs::write(project.join("cas-1/notes.txt"), png).unwrap();
        std::fs::write(project.join("cas-1/big.png"), [png.as_slice(), &vec![0; LOCAL_IMAGE_MAX_BYTES as usize]].concat()).unwrap();
        let outside = temp.path().join("secret.png");
        std::fs::write(&outside, png).unwrap();
        std::os::unix::fs::symlink(&outside, project.join("cas-1/link.png")).unwrap();
        std::fs::create_dir_all(artifacts.join("other-project")).unwrap();
        std::fs::write(artifacts.join("other-project/shot.png"), png).unwrap();
        let reference = |path: &str| format!("{LOCAL_IMAGE_PREFIX}{}", URL_SAFE_NO_PAD.encode(path));
        let absolute = project.join("cas-1/after/shot.png");
        for served in ["cas-1/after/shot.png", absolute.to_str().unwrap()] {
            let (record, _) = open_local_image(&root, &reference(served)).unwrap();
            assert_eq!((record.mime.as_str(), record.name.as_str(), record.size_bytes), ("image/png", "shot.png", png.len() as u64));
        }
        for refused in [
            "cas-1/../../other-project/shot.png",
            "../other-project/shot.png",
            outside.to_str().unwrap(),
            "/etc/passwd",
            "cas-1/link.png",
            "cas-1/page.png",
            "cas-1/notes.txt",
            "cas-1/big.png",
            "cas-1/missing.png",
            "cas-1",
        ] {
            assert!(open_local_image(&root, &reference(refused)).is_err(), "{refused} must be refused");
        }
        assert!(open_local_image(&root, "local-%2e%2e").is_err());
        assert!(open_local_image(&root, "art-shot").is_err());
    }
}
