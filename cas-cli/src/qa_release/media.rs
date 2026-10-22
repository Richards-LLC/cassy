//! Pack media for people: a Violet-postable bundle within Slack's per-file
//! limit, and local retention (Q6: media stay local, no Cloud upload).
//!
//! Every file is checked against the pack digest before it is posted, so
//! Violet receives exactly the bytes the pack names. Video over the limit is
//! re-encoded through a [`Transcoder`]; if that fails or is still too large,
//! the video stays local-only and the row's frames carry the evidence.
use super::pack::{MEDIA_DIR, Media, PACK_FILE, Pack, PackError, digest, media_path};
use crate::artifacts::slack::{PostError, PostOptions, PostReceipt, UploadFile, VioletPoster, post_bytes};
use chrono::{DateTime, Duration, Utc};
use std::path::{Path, PathBuf};

/// Transcoded copies live here, inside the pack's media directory.
const SLACK_DIR: &str = ".slack";
const VIDEO: &[&str] = &["mp4", "webm", "mov", "mkv"];

/// Re-encodes a video so it fits `max_bytes`. The production implementation
/// is [`Ffmpeg`]; tests use a fake.
pub trait Transcoder {
    fn transcode(&self, input: &Path, output: &Path, max_bytes: u64) -> Result<(), String>;
}

/// `ffmpeg`/`ffprobe` from PATH: H.264 at most 720p, no audio, with a bitrate
/// that targets 90% of the limit over the clip's duration.
pub struct Ffmpeg;

impl Transcoder for Ffmpeg {
    fn transcode(&self, input: &Path, output: &Path, max_bytes: u64) -> Result<(), String> {
        let run = |program: &str, args: &[&std::ffi::OsStr]| {
            let mut command = std::process::Command::new(program);
            command.args(args);
            let cap = std::time::Duration::from_secs(600);
            crate::bounded_process::run_command(&mut command, crate::bounded_process::Deadline::after(cap), cap)
                .map_err(|e| format!("{program}: {e:?}"))
                .and_then(|out| {
                    out.status.success().then_some(out.stdout).ok_or_else(|| format!("{program} exited {}", out.status))
                })
        };
        let probe = run(
            "ffprobe",
            &["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"]
                .map(std::ffi::OsStr::new)
                .into_iter()
                .chain([input.as_os_str()])
                .collect::<Vec<_>>(),
        )?;
        let seconds: f64 = String::from_utf8_lossy(&probe).trim().parse().map_err(|_| "ffprobe gave no duration")?;
        let kbps = ((max_bytes as f64 * 8.0 * 0.9) / seconds.max(1.0) / 1000.0).floor().max(64.0) as u64;
        let rate = format!("{kbps}k");
        let buffer = format!("{}k", kbps * 2);
        let mut args: Vec<&std::ffi::OsStr> = vec!["-y".as_ref(), "-v".as_ref(), "error".as_ref(), "-i".as_ref(), input.as_os_str()];
        args.extend(
            ["-an", "-vf", "scale=-2:'min(720,ih)'", "-c:v", "libx264", "-b:v", &rate, "-maxrate", &rate, "-bufsize", &buffer, "-movflags", "+faststart"]
                .map(std::ffi::OsStr::new),
        );
        args.push(output.as_os_str());
        run("ffmpeg", &args).map(|_| ())
    }
}

/// One file ready for Violet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleFile {
    pub path: PathBuf,
    /// `pack.json`, a pack media path, or `<media path> (transcoded)`.
    pub source: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Bundle {
    /// `pack.json` first, then media; each at most the limit.
    pub files: Vec<BundleFile>,
    /// Media that stay local: still over the limit after transcoding.
    pub local_only: Vec<Media>,
}

/// Plans the Slack copy of the sealed pack in `dir`, verifying it first. Each file is at most
/// `limit` bytes (Violet's artifact limit is [`cas_types::ARTIFACT_MAX_BYTES`]).
pub fn slack_bundle(dir: &Path, limit: u64, transcoder: &impl Transcoder) -> Result<Bundle, PackError> {
    let pack = Pack::load(dir)?;
    let mut bundle = Bundle::default();
    let index = dir.join(PACK_FILE);
    let (sha256, bytes) = digest(&index)?;
    if bytes > limit {
        return Err(PackError::Media(format!("{PACK_FILE} is {bytes} bytes, over the {limit}-byte post limit")));
    }
    bundle.files.push(BundleFile { path: index, source: PACK_FILE.into(), sha256, bytes });
    for media in &pack.media {
        let path = media_path(dir, &media.path)?;
        if media.bytes <= limit {
            bundle.files.push(BundleFile {
                path,
                source: media.path.clone(),
                sha256: media.sha256.clone(),
                bytes: media.bytes,
            });
            continue;
        }
        let video = path.extension().and_then(|e| e.to_str()).is_some_and(|e| VIDEO.contains(&e.to_ascii_lowercase().as_str()));
        let copy = transcoded_path(dir, media)?;
        let fits = video
            && transcoder.transcode(&path, &copy, limit).is_ok()
            && std::fs::symlink_metadata(&copy).is_ok_and(|m| m.file_type().is_file() && m.len() <= limit);
        if fits {
            let (sha256, bytes) = digest(&copy)?;
            bundle.files.push(BundleFile { path: copy, source: format!("{} (transcoded)", media.path), sha256, bytes });
        } else {
            let _ = std::fs::remove_file(&copy);
            bundle.local_only.push(media.clone());
        }
    }
    Ok(bundle)
}

fn transcoded_path(dir: &Path, media: &Media) -> Result<PathBuf, PackError> {
    let slack = dir.join(MEDIA_DIR).join(SLACK_DIR);
    match std::fs::symlink_metadata(&slack) {
        Ok(meta) if !meta.file_type().is_dir() => {
            return Err(PackError::Media(format!("{MEDIA_DIR}/{SLACK_DIR} is not a directory")));
        }
        Ok(_) => {}
        Err(_) => std::fs::create_dir_all(&slack).map_err(|e| PackError::Io(e.to_string()))?,
    }
    // Keyed by the media path: two videos with equal bytes are separate entries.
    let key = format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(media.path.as_bytes()));
    Ok(slack.join(format!("{}.mp4", &key[..16])))
}

/// Posts a bundle: `pack.json` with `summary` as its comment, then each file
/// in that message's thread. Bytes are re-hashed before each upload.
pub async fn post_pack(
    violet: &(impl VioletPoster + Sync),
    bundle: &Bundle,
    summary: &str,
    options: &PostOptions,
) -> Result<Vec<PostReceipt>, PostError> {
    let mut receipts: Vec<PostReceipt> = Vec::new();
    for (index, file) in bundle.files.iter().enumerate() {
        let bytes = std::fs::read(&file.path)
            .map_err(|e| PostError::new("pack_media_unreadable", format!("{}: {e}", file.source)))?;
        let actual = format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(&bytes));
        if actual != file.sha256 || bytes.len() as u64 != file.bytes {
            return Err(PostError::new("pack_media_changed", format!("{} differs from the pack digest", file.source)));
        }
        let name = file.path.file_name().and_then(|n| n.to_str()).unwrap_or("media");
        let mut options = options.clone();
        if index == 0 {
            options.initial_comment = Some(summary.into());
        } else {
            options.reply_to = receipts[0].message_id.clone().or(options.reply_to);
            options.title = Some(file.source.clone());
        }
        let upload = UploadFile { id: &file.source, name, size_bytes: file.bytes, sha256: &file.sha256 };
        receipts.push(post_bytes(violet, upload, bytes, &options).await?);
    }
    Ok(receipts)
}

/// Deletes media files older than `retention_days` (by modification time)
/// under each pack's own `media/` directory in `release_root`
/// (`<artifacts_root>/<project>/release`). Links are never followed or
/// deleted, and nothing outside a pack's `media/` is touched; `pack.json`
/// stays as the record. Returns the deleted paths.
pub fn prune_media(release_root: &Path, retention_days: u32, now: DateTime<Utc>) -> Vec<PathBuf> {
    let cutoff = now - Duration::days(i64::from(retention_days));
    let mut deleted = Vec::new();
    let Ok(packs) = std::fs::read_dir(release_root) else {
        return deleted;
    };
    for pack in packs.flatten() {
        let media = pack.path().join(MEDIA_DIR);
        if !std::fs::symlink_metadata(&media).is_ok_and(|m| m.file_type().is_dir())
            || !pack.file_type().is_ok_and(|t| t.is_dir())
        {
            continue;
        }
        for entry in walkdir::WalkDir::new(&media).follow_links(false).into_iter().flatten() {
            let Ok(meta) = entry.path().symlink_metadata() else { continue };
            let old = meta.modified().is_ok_and(|m| DateTime::<Utc>::from(m) < cutoff);
            if meta.file_type().is_file() && old && std::fs::remove_file(entry.path()).is_ok() {
                deleted.push(entry.path().to_path_buf());
            }
        }
    }
    deleted
}
