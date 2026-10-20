//! The operator's two hard Slack rules, checked on a `cas violet post|thread`
//! draft before anything is sent (cas-d47fd750acf1df00).
//!
//! The rules and their matching follow the Violet hub's `format_unreadable`
//! rules of the same names, so a draft refused here is refused the same way
//! there, and a refusal prints the same `detail.violations[]` shape:
//!
//! - `unattached_file`: a post never names a file the reader cannot open from
//!   Slack. It catches absolute or `~/` paths, relative paths with an
//!   extension, bare filenames with common extensions, and `art-xxxxxxxx`
//!   artifact ids, and passes only when the same message attaches a file of
//!   that name.
//! - `utc_time`: times are shown in CT (ET when labelled). It catches `UTC`,
//!   `GMT`, `HH:MMZ`, and ISO timestamps ending in `Z` or `+00:00`.
//!
//! Both apply to prose only: fenced code blocks and Slack `<url|label>` links
//! are exempt, and so are bare URLs, so this check is never stricter than the
//! hub.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use super::hub::VioletError;

/// One message of a draft: its text and the names of the files it attaches.
pub struct DraftMessage<'a> {
    /// Which message this is in a thread (`reply 2`), or `None` for a post.
    pub label: Option<String>,
    pub text: &'a str,
    pub attached: Vec<String>,
}

static LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<[^<>\s|]+(?:\|[^<>]*)?>").expect("link pattern"));
static URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"https?://\S+").expect("url pattern"));
static UTC_TIME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(?:UTC|GMT)\b|\b\d{1,2}:\d{2}(?::\d{2})?Z\b|\b\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}(?::\d{2}(?:\.\d+)?)?(?:Z|\+00:00)",
    )
    .expect("utc pattern")
});
static FILE_REFERENCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        // absolute (two or more segments) or home-relative paths
        r"(?:^|[^\w/~.])(?P<abs>(?:~|/[\w.\-]+)/[\w.\-]+(?:/[\w.\-]+)*)",
        // relative paths with an extension
        r"|\b(?P<rel>[\w.\-]+(?:/[\w.\-]+)+\.[A-Za-z0-9]{1,6})\b",
        // bare filenames with a common extension
        r"|\b(?P<bare>[\w\-]+(?:\.[\w\-]+)*\.(?:md|pdf|png|jpe?g|gif|svg|webp|txt|log|json|csv|tsv|html?|zip|tar|gz|tgz|ya?ml|toml|rs|py|sh|ts|tsx|js|jsx|mp4|mov|docx?|xlsx?|pptx?))\b",
        // Cassy artifact ids
        r"|\b(?P<art>art-[0-9a-f]{8})\b",
    ))
    .expect("file pattern")
});

const FIX_FILE: &str = "attach the file to this message (--file or --reply-file), or describe what it shows without naming it";
const FIX_TIME: &str = "show the time in US Central, for example 2:08 PM CT (ET is fine when labelled); never UTC, GMT or Z";

/// Every hard-rule violation in `message`, in line order, as hub-shaped
/// violation objects.
pub fn violations(message: &DraftMessage<'_>) -> Vec<Value> {
    let mut found = Vec::new();
    let mut in_fence = false;
    for (index, line) in message.text.lines().enumerate() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let prose = URL.replace_all(&LINK.replace_all(line, " "), " ").into_owned();
        let mut push = |rule: &str, fix: &str| {
            let mut violation = json!({
                "rule": rule,
                "line": index + 1,
                "excerpt": line.trim(),
                "fix": fix,
            });
            if let Some(label) = &message.label {
                violation["message"] = json!(label);
            }
            found.push(violation);
        };
        if UTC_TIME.is_match(&prose) {
            push("utc_time", FIX_TIME);
        }
        let unattached = FILE_REFERENCE.captures_iter(&prose).any(|captures| {
            let reference = ["abs", "rel", "bare", "art"]
                .iter()
                .find_map(|name| captures.name(name))
                .map(|matched| matched.as_str())
                .unwrap_or_default();
            let name = reference.rsplit('/').next().unwrap_or(reference);
            !message.attached.iter().any(|attached| attached == name)
        });
        if unattached {
            push("unattached_file", FIX_FILE);
        }
    }
    found
}

/// Refuse the draft before it is sent when any message breaks a hard rule.
/// The error carries the hub's `format_unreadable` envelope, so `--json` and
/// the plain output report it exactly as they report a hub refusal.
pub fn check(messages: &[DraftMessage<'_>]) -> Result<(), VioletError> {
    let found: Vec<Value> = messages.iter().flat_map(violations).collect();
    if found.is_empty() {
        return Ok(());
    }
    let message = format!(
        "the draft breaks {} Slack formatting rule(s); nothing was sent",
        found.len()
    );
    let mut error = VioletError::local("format_unreadable", message.clone());
    error.envelope = Some(json!({
        "ok": false,
        "error": {
            "code": "format_unreadable",
            "message": message,
            "retryable": false,
            "detail": { "violations": found },
        }
    }));
    Err(error)
}
