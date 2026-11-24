//! cas-d68f: park names, in one refusal, every symbol the delivery left
//! without callers, so a verifier round is not spent per orphaned helper.
//!
//! The sweep is lexical: one `git diff` of the delivery and one `git archive`
//! of its head, no build and no model. A symbol is a TS/JS function, method,
//! export or type, a Rust `pub` item, or a CSS class selector. A reference is
//! any other occurrence of its name; for a CSS class, any occurrence outside a
//! stylesheet. The delivery is blamed for a symbol with no reference at its
//! head only when the delivery removed its last reference: a new definition
//! with no caller yet is left to the compiler and linters. A class whose
//! prefix markup builds (`chip--${tone}`) is referenced.
//! Base counts are the head counts minus the added lines plus the removed
//! lines, so the base tree is never read, and a symbol already unreferenced at
//! the base is never blamed.
use regex::Regex;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::LazyLock;

/// Generated bundles and vendored code are not callers. `lang` picks the
/// sources; a pathspec per extension would fail where one matches nothing.
const PATHSPEC: &[&str] = &[
    ".", ":(exclude,glob)**/dist/**", ":(exclude,glob)**/node_modules/**",
    ":(exclude,glob)vendor/**", ":(exclude,glob)**/*.min.*",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lang {
    Rust,
    Script,
    Style,
}

fn lang(path: &str) -> Option<Lang> {
    match path.rsplit_once('.')?.1 {
        "rs" => Some(Lang::Rust),
        "css" | "scss" | "less" => Some(Lang::Style),
        "ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs" | "vue" | "svelte" | "html" => {
            Some(Lang::Script)
        }
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Kind {
    /// Function, method, export, type or Rust `pub` item.
    Code,
    Class,
}

static RUST_ITEM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^\s*pub(?:\([^)]*\))?\s+(?:(?:const|async|unsafe|extern\s+"[^"]*")\s+)*(?:fn|struct|enum|trait|type|const|static|union)\s+([A-Za-z_][A-Za-z0-9_]*)"#).unwrap()
});
static SCRIPT_ITEMS: LazyLock<[Regex; 4]> = LazyLock::new(|| {
    [
        r"^\s*(?:export\s+)?(?:default\s+)?(?:declare\s+)?(?:async\s+)?function\s*\*?\s*([A-Za-z_$][\w$]*)",
        r"^\s*(?:export\s+)?(?:declare\s+)?(?:abstract\s+)?(?:class|interface|type|enum)\s+([A-Za-z_$][\w$]*)",
        r"^\s*export\s+(?:const|let|var)\s+([A-Za-z_$][\w$]*)",
        r"^\s*(?:const|let)\s+([A-Za-z_$][\w$]*)\s*(?::[^=]*)?=\s*(?:async\s+)?(?:function\b|\([^)]*\)\s*(?::[^=]*)?=>|[A-Za-z_$][\w$]*\s*=>)",
    ]
    .map(|pattern| Regex::new(pattern).unwrap())
});
static SCRIPT_METHOD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:(?:public|private|protected|static|async|readonly|override|get|set)\s+)*#?([A-Za-z_$][\w$]*)\s*(?:<[^>]*>)?\s*\([^;]*\)\s*(?::\s*[^={;]+)?\{").unwrap()
});
static CLASS_SELECTOR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\.(-?[A-Za-z_][A-Za-z0-9_-]*)").unwrap());
const NOT_METHODS: &[&str] = &[
    "if", "for", "while", "switch", "catch", "function", "return", "with", "constructor",
    "super", "else", "do", "try", "typeof", "await", "new", "yield",
];

/// Definitions on one source line.
fn definitions(lang: Lang, line: &str) -> Vec<(String, Kind)> {
    let first = |regex: &Regex| regex.captures(line).map(|caps| caps[1].to_string());
    match lang {
        Lang::Rust => first(&RUST_ITEM).map(|name| vec![(name, Kind::Code)]).unwrap_or_default(),
        Lang::Script => SCRIPT_ITEMS.iter().find_map(first)
            .or_else(|| first(&SCRIPT_METHOD).filter(|name| !NOT_METHODS.contains(&name.as_str())))
            .map(|name| vec![(name, Kind::Code)])
            .unwrap_or_default(),
        Lang::Style => {
            // Only the selector part: declarations carry values like `.5em`.
            let selector = match line.split_once('{') {
                Some((selector, _)) => selector,
                None if line.trim_end().ends_with(',') => line,
                None => return Vec::new(),
            };
            CLASS_SELECTOR
                .captures_iter(selector)
                .map(|caps| (caps[1].to_string(), Kind::Class))
                .collect()
        }
    }
}

/// Maximal runs of `word` bytes that start with a non-digit.
fn tokens(line: &str, word: fn(u8) -> bool) -> impl Iterator<Item = &str> {
    line.split(move |c: char| !c.is_ascii() || !word(c as u8))
        .filter(|token| token.bytes().next().is_some_and(|byte| !byte.is_ascii_digit() && byte != b'-'))
}

/// Identifiers, except the words of a hyphenated name: `status-chip` is CSS
/// class text, not a reference to `status` or `chip`.
fn identifiers(line: &str) -> impl Iterator<Item = &str> {
    let bytes = line.as_bytes();
    let joined = move |hyphen: Option<usize>, word: Option<usize>| {
        hyphen.and_then(|at| bytes.get(at)) == Some(&b'-')
            && word.and_then(|at| bytes.get(at)).is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
    };
    tokens(line, ident_byte).filter(move |token| {
        let start = token.as_ptr() as usize - line.as_ptr() as usize;
        let end = start + token.len();
        !joined(start.checked_sub(1), start.checked_sub(2)) && !joined(Some(end), Some(end + 1))
    })
}

fn ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$'
}

fn class_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
}

/// Occurrences of one name, signed so diff lines can move head counts to base.
#[derive(Clone, Copy, Default)]
struct Counts {
    ident: i64,
    class_ref: i64,
    defs: i64,
}

impl Counts {
    fn refs(&self, kind: Kind) -> i64 {
        match kind {
            Kind::Class => self.class_ref,
            Kind::Code => self.ident - self.defs,
        }
    }
}

fn tally(counts: &mut HashMap<String, Counts>, names: &HashSet<String>, lang: Lang, line: &str, defs: &[(String, Kind)], sign: i64) {
    let mut add = |name: &str, apply: &dyn Fn(&mut Counts)| {
        if names.contains(name) {
            apply(counts.entry(name.to_string()).or_default());
        }
    };
    for (name, kind) in defs {
        match kind {
            Kind::Code => add(name, &|c| c.defs += sign),
            Kind::Class => {}
        }
    }
    if lang != Lang::Style {
        for token in identifiers(line) {
            add(token, &|c| c.ident += sign);
        }
        for token in tokens(line, class_byte) {
            add(token, &|c| c.class_ref += sign);
        }
    }
}

/// Rust fixtures embed source in raw strings; their lines define nothing.
/// Returns whether `line` starts inside one and tracks whether it ends inside.
fn raw_string_state(line: &str, open: &mut Option<usize>) -> bool {
    let inside = open.is_some();
    let bytes = line.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        if let Some(hashes) = *open {
            let close = format!("\"{}", "#".repeat(hashes));
            let Some(end) = line[at..].find(&close) else { break };
            *open = None;
            at += end + close.len();
            continue;
        }
        let starts_raw = bytes[at] == b'r'
            && (at == 0 || !ident_byte(bytes[at - 1])
                || (bytes[at - 1] == b'b' && (at == 1 || !ident_byte(bytes[at - 2]))));
        if starts_raw {
            let hashes = bytes[at + 1..].iter().take_while(|byte| **byte == b'#').count();
            if bytes.get(at + 1 + hashes) == Some(&b'"') {
                *open = Some(hashes);
                at += hashes + 2;
                continue;
            }
        }
        at += 1;
    }
    inside
}

struct Diff {
    /// Removed lines add and added lines subtract: head + delta = base.
    delta: HashMap<String, Counts>,
    candidates: HashSet<String>,
}

fn git(repo: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|error| format!("git {}: {error}", args[0]))?;
    if !output.status.success() {
        return Err(format!("git {}: {}", args[0], String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(output.stdout)
}

fn read_diff(repo: &Path, base: &str, head: &str) -> Result<Diff, String> {
    let mut args = vec!["diff", "--no-color", "--no-ext-diff", "--no-renames", "-U0", base, head, "--"];
    args.extend(PATHSPEC);
    let text = String::from_utf8_lossy(&git(repo, &args)?).into_owned();
    // Lines are judged once the candidate set is known.
    let mut lines: Vec<(Lang, &str, i64)> = Vec::new();
    let mut candidates = HashSet::new();
    let mut file_lang = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("+++ ") {
            file_lang = rest.strip_prefix("b/").and_then(lang);
        } else if line.starts_with("--- ") || line.starts_with("diff ") || line.starts_with("index ") || line.starts_with("@@ ") {
        } else if let (Some(lang), Some(body)) = (file_lang, line.strip_prefix('+')) {
            lines.push((lang, body, -1));
        } else if let (Some(lang), Some(body)) = (file_lang, line.strip_prefix('-')) {
            // A lost reference is a removed occurrence outside a stylesheet.
            if lang != Lang::Style {
                candidates.extend(identifiers(body).map(str::to_string));
                candidates.extend(tokens(body, class_byte).map(str::to_string));
            }
            lines.push((lang, body, 1));
        }
    }
    let mut delta = HashMap::new();
    for (lang, body, sign) in lines {
        tally(&mut delta, &candidates, lang, body, &definitions(lang, body), sign);
    }
    Ok(Diff { delta, candidates })
}

/// One symbol the delivery left without a reference.
#[derive(Debug, PartialEq, Eq)]
struct Orphan {
    path: String,
    line: usize,
    name: String,
    kind: Kind,
}

fn orphans(repo: &Path, base: &str, head: &str) -> Result<Vec<Orphan>, String> {
    let diff = read_diff(repo, base, head)?;
    if diff.candidates.is_empty() {
        return Ok(Vec::new());
    }
    let mut args = vec!["archive", "--format=tar", head, "--"];
    args.extend(PATHSPEC);
    let mut child = Command::new("git")
        .args(&args)
        .current_dir(repo)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("git archive: {error}"))?;
    let mut counts = HashMap::new();
    let mut defined: BTreeMap<(String, Kind), (String, usize)> = BTreeMap::new();
    // `status-chip--${tone}` reaches every `status-chip--*` class.
    let mut class_prefixes = HashSet::new();
    let mut archive = tar::Archive::new(child.stdout.take().ok_or("git archive: no output")?);
    for entry in archive.entries().map_err(|error| format!("git archive: {error}"))? {
        let mut entry = entry.map_err(|error| format!("git archive: {error}"))?;
        let path = entry.path().map_err(|error| format!("git archive: {error}"))?.to_string_lossy().into_owned();
        let Some(file_lang) = lang(&path) else { continue };
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).map_err(|error| format!("git archive: {error}"))?;
        let Ok(body) = String::from_utf8(bytes) else { continue };
        let mut raw = None;
        for (index, line) in body.lines().enumerate() {
            let in_fixture = file_lang == Lang::Rust && raw_string_state(line, &mut raw);
            let defs = if in_fixture { Vec::new() } else { definitions(file_lang, line) };
            tally(&mut counts, &diff.candidates, file_lang, line, &defs, 1);
            if file_lang != Lang::Style {
                class_prefixes.extend(
                    tokens(line, class_byte).filter(|token| token.len() > 1 && token.ends_with('-')).map(str::to_string),
                );
            }
            for (name, kind) in defs {
                if diff.candidates.contains(&name) {
                    defined.entry((name, kind)).or_insert_with(|| (path.clone(), index + 1));
                }
            }
        }
    }
    let status = child.wait().map_err(|error| format!("git archive: {error}"))?;
    if !status.success() {
        return Err(format!("git archive {head} failed"));
    }
    let mut found: Vec<Orphan> = defined
        .into_iter()
        .filter_map(|((name, kind), (path, line))| {
            let head_counts = counts.get(&name).copied().unwrap_or_default();
            if head_counts.refs(kind) > 0
                || (kind == Kind::Class && class_prefixes.iter().any(|prefix: &String| name.starts_with(prefix.as_str())))
            {
                return None;
            }
            let delta = diff.delta.get(&name).copied().unwrap_or_default();
            let base = Counts {
                ident: head_counts.ident + delta.ident,
                class_ref: head_counts.class_ref + delta.class_ref,
                defs: head_counts.defs + delta.defs,
            };
            (base.refs(kind) > 0).then_some(Orphan { path, line, name, kind })
        })
        .collect();
    found.sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
    Ok(found)
}

/// Park check: `Ok(Some(note))` records why the sweep could not measure.
pub(super) fn at_park(repo: &Path, target: &str, head: Option<&str>) -> Result<Option<String>, String> {
    let Some(head) = head else { return Ok(None) };
    let target = super::freshest_target_ref(repo, target);
    let Some(base) = super::git_merge_base(repo, head, &target) else {
        return Ok(Some(format!("Dead-code sweep skipped: no merge base between {head} and {target}.")));
    };
    let found = match orphans(repo, &base, head) {
        Ok(found) => found,
        Err(error) => return Ok(Some(format!("Dead-code sweep skipped: {error}."))),
    };
    if found.is_empty() {
        return Ok(None);
    }
    let list = found
        .iter()
        .map(|orphan| {
            let kind = match orphan.kind {
                Kind::Code => "definition",
                Kind::Class => "CSS class",
            };
            format!("- {}:{} {} ({kind}): the delivery removed its last reference", orphan.path, orphan.line, orphan.name)
        })
        .collect::<Vec<_>>()
        .join("\n");
    Err(format!(
        "DEAD CODE LEFT BY THE DELIVERY: {} symbol(s) at {} have no caller.\n{list}\n\nNext: delete each one, or keep the caller it still needs, then retry close. A name reached only at runtime needs a supervisor override with its reason.",
        found.len(),
        &head[..head.len().min(12)],
    ))
}
