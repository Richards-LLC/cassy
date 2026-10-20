//! The operator's two hard Slack rules, checked on a `cas violet post|thread`
//! draft before anything is sent (cas-d47fd750acf1df00).
//!
//! This is a port of the Violet hub's own lint for those two rules
//! (violet_ps `apps/violet/lib/readability.ts`, `lintSlackText`, at
//! e0be0d043a5a), so a draft refused here is refused there, with the same
//! `format_unreadable` envelope, violation fields, excerpt and fix text:
//!
//! - `unattached_file`: a post never names a file the reader cannot open from
//!   Slack. It catches `~/` paths and paths under `/home`, `/tmp`, `/mnt`,
//!   `/Users` or `/var`; relative paths (a slash plus a known extension); bare
//!   filenames with a document or data extension; and `art-xxxxxxxx` artifact
//!   ids. A relative code path (`.ts`, `.js`, `.py`, `.rs`) counts only with
//!   two or more slashes or an all-lowercase first segment, so `src/App.ts`
//!   counts while `React/three.js` and `Svelte/d3.js` pass.
//!   It passes when the same message attaches a file of that name, compared
//!   case-insensitively. Backticks do not exempt a name.
//! - `utc_time`: times are shown in CT (ET when labelled). It catches `UTC`,
//!   `GMT`, `HH:MMZ`, and ISO timestamps ending in `Z` or `+00:00`.
//!
//! Fenced code is never checked, and URLs (bare or in `<…>`) never count as
//! file names. The hub's other seven rules are enforced by the hub; their
//! refusals print through the same envelope.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use super::hub::VioletError;

/// The hub's `format_unreadable` message, verbatim.
pub const FORMAT_UNREADABLE_MESSAGE: &str = "Rewrite the post for readability, then submit it again.";

const EXCERPT: usize = 80;
const EXCERPT_CONTEXT: usize = 20;
const CODE_PATH_SLASHES: usize = 2;

/// One message of a draft: its text and the names of the files it attaches.
pub struct DraftMessage<'a> {
    /// Which message this is in a thread (`reply 2`), or `None` for a post.
    pub label: Option<String>,
    pub text: &'a str,
    pub attached: Vec<String>,
}

const DOCUMENT_EXTENSIONS: &str = "md|html|pdf|png|jpg|jpeg|gif|svg|csv|json|txt|log|zip|yaml|toml";
const FILE_EXTENSIONS: &str = "md|html|pdf|png|jpg|jpeg|gif|svg|csv|json|txt|log|zip|yaml|toml|ts|js|py|rs";

static LABELLED_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<[^<>\n|]+\|([^<>\n]*)>").expect("link pattern"));
static URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)<[a-z][\w+.\-]*:[^<>\n]+>|\b[a-z][\w+.\-]*://[^\s<>`]+|\b(?:mailto|tel):[^\s<>`]+|\b(?:[a-z0-9\-]+\.)+[a-z]{2,}(?::\d+)?/[^\s<>`]*",
    )
    .expect("url pattern")
});
static UTC_TIME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:UTC|GMT)\b|\b\d{1,2}:\d{2}(?::\d{2}(?:\.\d+)?)?Z\b|\b\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(?::\d{2}(?:\.\d+)?)?(?:Z|\+00:00)\b",
    )
    .expect("utc pattern")
});
static CODE_EXTENSION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\.(?:ts|js|py|rs)$").expect("code extension pattern"));

/// The four `unattachedFile` alternatives, in the hub's order, each with the
/// characters its lookbehind refuses before the match (`regex` has no
/// lookbehind, so it is checked by hand).
static FILE_ALTERNATIVES: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    vec![
        (
            Regex::new(r#"(?i)(?:~/|/(?:home|tmp|mnt|Users|var)/)[^\s`<>"'()\[\]{},;!?]*"#)
                .expect("absolute path pattern"),
            "",
        ),
        (
            Regex::new(&format!(
                r"(?i)[A-Za-z0-9_@.\-]+(?:/[A-Za-z0-9_@.\-]+)+\.(?:{FILE_EXTENSIONS})\b(?::\d+(?::\d+)?)?"
            ))
            .expect("relative path pattern"),
            ".",
        ),
        (
            Regex::new(&format!(
                r"(?i)[A-Za-z0-9_@\-][A-Za-z0-9_@.\-]*\.(?:{DOCUMENT_EXTENSIONS})\b(?::\d+(?::\d+)?)?"
            ))
            .expect("bare filename pattern"),
            "./",
        ),
        (
            Regex::new(r"(?i)\bart-[0-9a-f]{8}\b").expect("artifact id pattern"),
            "",
        ),
    ]
});

fn is_ascii_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// The next match of one alternative at or after `from` whose preceding
/// character passes its lookbehind (`(?<![\w<extra>])`, when `extra` is set).
fn next_alternative(text: &str, from: usize, pattern: &Regex, extra: &str) -> Option<(usize, usize)> {
    let mut start = from;
    while start <= text.len() {
        let found = pattern.find_at(text, start)?;
        let refused = !extra.is_empty()
            && text[..found.start()]
                .chars()
                .next_back()
                .is_some_and(|c| is_ascii_word(c) || extra.contains(c));
        if !refused {
            return Some((found.start(), found.end()));
        }
        start = found.start() + text[found.start()..].chars().next().map_or(1, char::len_utf8);
    }
    None
}

/// Every `unattachedFile` match in `text`, scanning left to right the way a
/// global JavaScript regex does: at each position the earliest-starting
/// alternative wins, ties going to the earlier alternative.
fn file_references(text: &str) -> Vec<(usize, &str)> {
    let mut found = Vec::new();
    let mut position = 0;
    while position <= text.len() {
        let next = FILE_ALTERNATIVES
            .iter()
            .filter_map(|(pattern, extra)| next_alternative(text, position, pattern, extra))
            .min_by_key(|(start, _)| *start);
        let Some((start, end)) = next else { break };
        found.push((start, &text[start..end]));
        position = if end > start {
            end
        } else {
            end + text[end..].chars().next().map_or(1, char::len_utf8)
        };
    }
    found
}

/// The hub's `basename`: drop trailing dots, a `:line[:col]` suffix and
/// trailing slashes, then keep the last path segment.
fn basename(reference: &str) -> String {
    static POSITION: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r":\d+(?::\d+)?$").expect("position pattern"));
    let trimmed = reference.trim_end_matches('.');
    let without_position = POSITION.replace(trimmed, "").into_owned();
    let without_slashes = without_position.trim_end_matches('/');
    without_slashes.rsplit('/').next().unwrap_or(without_slashes).to_string()
}

fn excerpt(source: &str, byte_index: usize) -> String {
    let start = source[..byte_index].chars().count().saturating_sub(EXCERPT_CONTEXT);
    source.chars().skip(start).take(EXCERPT).collect()
}

/// One physical line, as the hub lints it.
struct LintLine {
    number: usize,
    text: String,
    file_prose: String,
}

fn lint_lines(text: &str) -> Vec<LintLine> {
    let leading = &text[..text.len() - text.trim_start().len()];
    let offset = leading.matches('\n').count();
    let mut in_code = false;
    text.trim()
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .enumerate()
        .map(|(index, original)| {
            let mut visible = String::new();
            let mut start = 0;
            for (fence, _) in original.match_indices("```") {
                if !in_code {
                    visible.push_str(&original[start..fence]);
                }
                in_code = !in_code;
                start = fence + 3;
            }
            if !in_code {
                visible.push_str(&original[start..]);
            }
            LintLine {
                number: index + offset + 1,
                text: LABELLED_LINK.replace_all(&visible, "$1").into_owned(),
                file_prose: URL.replace_all(&visible, "").into_owned(),
            }
        })
        .collect()
}

/// The `unattached_file` and `utc_time` violations in `message`, in the hub's
/// order and shape.
pub fn violations(message: &DraftMessage<'_>) -> Vec<Value> {
    let attached: Vec<String> = message
        .attached
        .iter()
        .map(|name| basename(name).to_lowercase())
        .collect();
    let mut found = Vec::new();
    for line in lint_lines(message.text) {
        if line.text.trim().is_empty() {
            continue;
        }
        for (index, reference) in file_references(&line.file_prose) {
            let name = basename(reference);
            let explicit = reference.starts_with("~/") || reference.starts_with('/');
            // A one-slash code reference names a file only when its directory
            // segment is all lowercase; `React/three.js` is a stack pair.
            let first_segment = reference.split('/').next().unwrap_or(reference);
            let lowercase_directory = first_segment == first_segment.to_lowercase();
            if CODE_EXTENSION.is_match(&name)
                && !explicit
                && reference.matches('/').count() < CODE_PATH_SLASHES
                && !lowercase_directory
            {
                continue;
            }
            if !attached.contains(&name.to_lowercase()) {
                found.push(json!({
                    "rule": "unattached_file",
                    "line": line.number,
                    "excerpt": excerpt(&line.file_prose, index),
                    "fix": format!("Attach \"{reference}\" to this message, or describe it without naming a path."),
                }));
            }
        }
        for time in UTC_TIME.find_iter(&line.text) {
            found.push(json!({
                "rule": "utc_time",
                "line": line.number,
                "excerpt": excerpt(&line.text, time.start()),
                "fix": format!("Show \"{}\" in CT (or labelled ET), never UTC.", time.as_str()),
            }));
        }
    }
    found
}

/// Refuse the draft before it is sent when a message breaks a hard rule. The
/// error carries the hub's `format_unreadable` envelope for the first such
/// message, so `--json` and the plain output read exactly like a hub refusal;
/// the plain message also names which thread message it was.
pub fn check(messages: &[DraftMessage<'_>]) -> Result<(), VioletError> {
    for message in messages {
        let found = violations(message);
        if found.is_empty() {
            continue;
        }
        let summary = match &message.label {
            Some(label) => format!("{FORMAT_UNREADABLE_MESSAGE} ({label})"),
            None => FORMAT_UNREADABLE_MESSAGE.to_string(),
        };
        let mut error = VioletError::local("format_unreadable", summary);
        error.envelope = Some(json!({
            "ok": false,
            "error": {
                "code": "format_unreadable",
                "message": FORMAT_UNREADABLE_MESSAGE,
                "retryable": false,
                "detail": { "violations": found },
            }
        }));
        return Err(error);
    }
    Ok(())
}
