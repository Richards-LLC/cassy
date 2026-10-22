//! Pack media for people: a Violet-postable bundle within Slack's per-file
//! limit, and local retention (Q6: media stay local, no Cloud upload).
//!
//! Every file is checked against the pack digest before it is posted, so
//! Violet receives exactly the bytes the pack names. Video over the limit is
//! re-encoded through a [`Transcoder`]; if that fails or is still too large,
//! the video stays local-only and the row's frames carry the evidence.
use super::pack::{MEDIA_DIR, Media, PACK_FILE, Pack, PackError, Pruned, digest, media_path, pruned, record_pruned};
use crate::artifacts::slack::{PostError, PostOptions, PostReceipt, UploadFile, VioletPoster, post_bytes};
use chrono::{DateTime, Duration, Utc};
use std::path::{Path, PathBuf};

/// Transcoded copies live here, inside the pack's media directory.
const SLACK_DIR: &str = ".slack";
const VIDEO: &[&str] = &["mp4", "webm", "mov", "mkv"];
/// Only images and video are uploaded, by extension and magic bytes; traces,
/// logs and archives can carry credentials, so the Slack post links them by
/// local path instead.
const IMAGE: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

fn extension_in(path: &str, list: &[&str]) -> bool {
    Path::new(path).extension().and_then(|e| e.to_str()).is_some_and(|e| list.contains(&e.to_ascii_lowercase().as_str()))
}

/// Whether the file's leading bytes are an image (`video == false`) or a
/// video container: the extension alone would upload a zip named `x.png`.
fn sniffed(path: &Path, video: bool) -> bool {
    use std::io::Read;
    // Up to 12 bytes or EOF: a single read() may return short.
    let mut head = Vec::with_capacity(12);
    if std::fs::File::open(path).and_then(|file| file.take(12).read_to_end(&mut head)).is_err() {
        return false;
    }
    let head = head.as_slice();
    if video {
        // MP4/MOV: an `ftyp` box with a video major brand (HEIC, AVIF and
        // M4A share the box); WebM/Matroska: the EBML header.
        const BRANDS: &[&[u8; 4]] = &[b"isom", b"iso2", b"iso4", b"iso5", b"iso6", b"mp41", b"mp42", b"avc1", b"qt  ", b"M4V ", b"dash"];
        (head.get(4..8) == Some(b"ftyp") && head.get(8..12).is_some_and(|brand| BRANDS.iter().any(|b| b.as_slice() == brand)))
            || head.starts_with(&[0x1A, 0x45, 0xDF, 0xA3])
    } else {
        head.starts_with(b"\x89PNG\r\n\x1a\n")
            || head.starts_with(&[0xFF, 0xD8, 0xFF])
            || head.starts_with(b"GIF87a")
            || head.starts_with(b"GIF89a")
            || (head.starts_with(b"RIFF") && head.get(8..12) == Some(b"WEBP"))
    }
}

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
    /// Media that are not image or video: never uploaded, named by local path.
    pub linked: Vec<Media>,
    /// Media retention already removed.
    pub pruned: Vec<Media>,
}

/// Plans the Slack copy of the sealed pack in `dir`, verifying it first.
/// Only `pack.json` (secret-scanned at seal), images and video are uploaded. Each file is at most
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
    let pruned = pruned(dir)?;
    for media in &pack.media {
        if pruned.iter().any(|p| p.path == media.path) && std::fs::symlink_metadata(dir.join(&media.path)).is_err() {
            bundle.pruned.push(media.clone());
            continue;
        }
        let path = media_path(dir, &media.path)?;
        let video = extension_in(&media.path, VIDEO);
        if !(video || extension_in(&media.path, IMAGE)) || !sniffed(&path, video) {
            bundle.linked.push(media.clone());
            continue;
        }
        if media.bytes <= limit {
            bundle.files.push(BundleFile {
                path,
                source: media.path.clone(),
                sha256: media.sha256.clone(),
                bytes: media.bytes,
            });
            continue;
        }
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

/// Posts a bundle: `pack.json` with `summary` as its comment (naming media
/// kept local), then each file in that message's thread. Bytes are re-hashed
/// before each upload.
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
            let local: Vec<&str> =
                bundle.linked.iter().chain(&bundle.local_only).map(|m| m.path.as_str()).collect();
            options.initial_comment = Some(if local.is_empty() {
                summary.into()
            } else {
                format!("{summary}\nKept local (not uploaded): {}", local.join(", "))
            });
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
/// deleted, and nothing outside a pack's `media/` is deleted. Each deleted
/// pack entry is first recorded in the pack's `pruned.json`, so `pack.json`
/// (whose bytes never change) stays loadable as the record. A pack whose
/// `pack.json` exists but does not parse is left alone. Returns the deleted paths.
pub fn prune_media(release_root: &Path, retention_days: u32, now: DateTime<Utc>) -> Vec<PathBuf> {
    let cutoff = now - Duration::days(i64::from(retention_days));
    let mut deleted = Vec::new();
    let Ok(packs) = std::fs::read_dir(release_root) else {
        return deleted;
    };
    for pack in packs.flatten() {
        let dir = pack.path();
        let media = dir.join(MEDIA_DIR);
        if !std::fs::symlink_metadata(&media).is_ok_and(|m| m.file_type().is_dir())
            || !pack.file_type().is_ok_and(|t| t.is_dir())
        {
            continue;
        }
        let listed: Vec<Media> = match std::fs::read_to_string(dir.join(PACK_FILE)) {
            Ok(text) => match serde_json::from_str::<Pack>(&text) {
                Ok(sealed) => sealed.media,
                Err(_) => continue,
            },
            Err(_) => Vec::new(),
        };
        let old: Vec<PathBuf> = walkdir::WalkDir::new(&media)
            .follow_links(false)
            .into_iter()
            .flatten()
            .filter(|entry| {
                entry.path().symlink_metadata().is_ok_and(|meta| {
                    meta.file_type().is_file() && meta.modified().is_ok_and(|m| DateTime::<Utc>::from(m) < cutoff)
                })
            })
            .map(|entry| entry.path().to_path_buf())
            .collect();
        // A pack entry is pruned only while its bytes still match: a changed
        // file stays, so loading keeps refusing it rather than forgetting it.
        let mut old = old;
        let mut records = Vec::new();
        for media in &listed {
            let path = dir.join(&media.path);
            if !old.contains(&path) {
                continue;
            }
            if digest(&path).is_ok_and(|(sha256, bytes)| sha256 == media.sha256 && bytes == media.bytes) {
                records.push(Pruned { path: media.path.clone(), sha256: media.sha256.clone(), bytes: media.bytes, pruned_at: now });
            } else {
                old.retain(|p| p != &path);
            }
        }
        if !records.is_empty() && record_pruned(&dir, &records).is_err() {
            continue;
        }
        for path in old {
            if std::fs::remove_file(&path).is_ok() {
                deleted.push(path);
            }
        }
    }
    deleted
}
