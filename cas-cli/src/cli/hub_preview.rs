//! `cas hub preview` (cas-93d1): show a candidate Commander build against the
//! machine's live data without letting it change anything.
//!
//! A browser device is bound to the origin it paired from, so the candidate
//! runs on its own loopback origin and is paired once through a preview
//! invitation. The hub gives that device read-only scopes, refuses every
//! mutation route to it whatever scope the route names, and stops
//! authenticating it when the preview window ends, refresh included. This
//! server only serves the candidate's static files; it never proxies the hub.
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Args;

use crate::cli::Cli;
use crate::hub::{HubRuntimePaths, Scope};

#[derive(Args, Debug, Clone)]
pub struct HubPreviewArgs {
    /// A built Commander directory (hub-web/dist) containing index.html
    #[arg(long)]
    pub dist: PathBuf,
    /// Loopback port to serve on (0 picks a free one)
    #[arg(long, default_value_t = 0)]
    pub port: u16,
    /// How long the preview origin may read live data, in minutes (1–480)
    #[arg(long, default_value_t = 120, value_parser = clap::value_parser!(u32).range(1..=480))]
    pub minutes: u32,
    /// Hub address the pairing link prefills (defaults as for `cas hub pair`)
    #[arg(long)]
    pub hub_url: Option<String>,
    /// The parked task this build delivers; Commander links its row here
    #[arg(long, value_parser = task_id)]
    pub task: Option<String>,
}

fn task_id(value: &str) -> std::result::Result<String, String> {
    let valid = (1..=64).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    valid.then(|| value.to_owned()).ok_or_else(|| "a task id is 1–64 letters, digits, '-', '_' or '.'".into())
}

/// The candidate file a request path names, or None. Only `/`, `/index.html`
/// and `/commander/<file>` resolve, and only to a regular file inside `dist`.
pub(crate) fn preview_file(dist: &Path, request_path: &str) -> Option<PathBuf> {
    let path = request_path.split(['?', '#']).next().unwrap_or("");
    let relative = match path {
        "/" | "/index.html" | "/commander" | "/commander/" => "index.html",
        other => other.strip_prefix("/commander/")?,
    };
    let decoded = urlencoding::decode(relative).ok()?;
    let relative = Path::new(decoded.as_ref());
    if relative.components().any(|part| !matches!(part, Component::Normal(_))) {
        return None;
    }
    let root = dist.canonicalize().ok()?;
    let file = root.join(relative).canonicalize().ok()?;
    (file.starts_with(&root) && file.is_file()).then_some(file)
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|value| value.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "webmanifest" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "woff2" => "font/woff2",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

fn header(name: &str, value: &str) -> tiny_http::Header {
    tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("static header")
}

fn respond(request: tiny_http::Request, dist: &Path) -> std::io::Result<()> {
    let method = request.method().clone();
    if !matches!(method, tiny_http::Method::Get | tiny_http::Method::Head) {
        return request.respond(tiny_http::Response::empty(405).with_header(header("Allow", "GET, HEAD")));
    }
    let Some(file) = preview_file(dist, request.url()) else {
        return request.respond(tiny_http::Response::empty(404));
    };
    let mut body = Vec::new();
    std::fs::File::open(&file)?.read_to_end(&mut body)?;
    let response = tiny_http::Response::from_data(if method == tiny_http::Method::Head { Vec::new() } else { body })
        .with_header(header("Content-Type", content_type(&file)))
        .with_header(header("Cache-Control", "no-store"))
        .with_header(header("X-Content-Type-Options", "nosniff"))
        .with_header(header("Referrer-Policy", "no-referrer"));
    request.respond(response)
}

pub(super) fn preview(args: &HubPreviewArgs, cli: &Cli) -> Result<()> {
    let dist = args.dist.canonicalize().with_context(|| format!("no Commander build at {}", args.dist.display()))?;
    anyhow::ensure!(dist.join("index.html").is_file(), "{} has no index.html; build hub-web first (npm run build)", dist.display());
    let listener = std::net::TcpListener::bind(("127.0.0.1", args.port)).context("cannot bind the preview port")?;
    let origin = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    let scopes: Vec<Scope> = Scope::default_read_only().into_iter().collect();
    // The same operator-only gate as `cas hub pair`: only the operator mints
    // an invitation for their own machine hub, after confirming it.
    let paths = HubRuntimePaths::default_for_user()?;
    if super::hub::pairing_targets_operator_hub(paths.root(), super::hub::account_home().as_deref()) {
        super::hub::pairing_admission(
            &crate::config::operator_policy::InvocationContext::from_process(),
            &origin,
            &scopes,
            &mut |summary| {
                eprintln!("{summary}\nPreview: read-only for {} minutes.", args.minutes);
                inquire::Confirm::new("Mint this one-time preview pairing invitation?").with_default(false).prompt().unwrap_or(false)
            },
        )
        .map_err(anyhow::Error::msg)?;
    }
    let now = chrono::Utc::now();
    let until = now + chrono::Duration::minutes(i64::from(args.minutes));
    let configured_hub_url = crate::store::find_cas_root()
        .ok()
        .and_then(|cas_root| crate::config::Config::load(&cas_root).ok())
        .and_then(|config| config.hub.and_then(|hub| hub.public_url));
    let prefill = crate::hub::PairingPrefill {
        hub_url: super::hub_reverse_pairing::pairing_prefill_hub_url(&paths, args.hub_url.as_deref(), configured_hub_url.as_deref())?,
        machine_label: Some(super::hub_reverse_pairing::machine_display_label()),
    };
    let store = super::hub::auth_store()?;
    let invitation = store.mint_preview_pairing(&origin, args.task.as_deref(), until, now)?.with_prefill(prefill);
    if cli.json {
        println!("{}", serde_json::json!({ "origin": origin, "url": invitation.url, "until": until, "dist": dist, "task": args.task }));
    } else {
        println!("Previewing {} at {origin}/ until {} (read-only)", dist.display(), until.with_timezone(&chrono::Local).format("%-I:%M %p"));
        println!("Pair this preview once, then open it:\n{}", invitation.url);
    }
    let server = tiny_http::Server::from_listener(listener, None).map_err(|error| anyhow::anyhow!("preview server: {error}"))?;
    let deadline = Instant::now() + Duration::from_secs(u64::from(args.minutes) * 60);
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        if let Some(request) = server.recv_timeout(left.min(Duration::from_secs(1)))? {
            let _ = respond(request, &dist);
        }
    }
    // The devices stop authenticating at `until` on their own; this also
    // revokes them and any unused invitation now.
    store.end_preview(&origin, chrono::Utc::now())?;
    println!("Preview ended; its pairing is revoked.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_only_files_inside_the_candidate_dist() {
        let temp = tempfile::tempdir().unwrap();
        let dist = temp.path().join("dist");
        std::fs::create_dir_all(&dist).unwrap();
        std::fs::write(dist.join("index.html"), "<!doctype html>").unwrap();
        std::fs::write(dist.join("app.js"), "1").unwrap();
        std::fs::write(temp.path().join("secret"), "x").unwrap();
        std::os::unix::fs::symlink(temp.path().join("secret"), dist.join("link.js")).unwrap();
        for (path, served) in [("/", Some("index.html")), ("/commander/", Some("index.html")), ("/commander/app.js?v=1", Some("app.js"))] {
            assert_eq!(preview_file(&dist, path).and_then(|file| file.file_name().map(|name| name.to_string_lossy().into_owned())).as_deref(), served, "{path}");
        }
        for path in ["/commander/../secret", "/commander/%2e%2e/secret", "/commander/link.js", "/secret", "/commander/missing.js", "/v1/sessions"] {
            assert_eq!(preview_file(&dist, path), None, "{path}");
        }
    }
}
