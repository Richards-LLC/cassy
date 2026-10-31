//! Release-supported retirement, deliberately separate from Config::save:
//! unknown values and explicit defaults must survive an update byte for byte.
use super::io::{atomic_replace_project_config, lock_project_config};
use crate::config::Config;
use serde::Serialize;
use std::{
    fs,
    io::Write,
    ops::Range,
    path::{Path, PathBuf},
};
use toml_edit::{Decor, Item, RawString, Table, Value};

#[derive(Debug, Serialize)]
pub(crate) struct ConfigRetirement {
    pub path: PathBuf,
    pub status: &'static str,
    pub removed: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Missing/current files cause no writes, including lock/backup creation.
/// Re-read under the same lock as every other config writer before replacing.
pub(crate) fn retire_config(root: &Path, dry_run: bool) -> ConfigRetirement {
    let mut receipt = ConfigRetirement {
        path: root.join("config.toml"),
        status: "unchanged",
        removed: Vec::new(),
        backup: None,
        error: None,
    };
    let result = (|| -> Result<(), String> {
        let Some(before) = read_config(&receipt.path)? else {
            receipt.status = "missing";
            return Ok(());
        };
        let (_, removed) = retirement_edit(&before)?;
        if removed.is_empty() {
            return Ok(());
        }
        if dry_run {
            receipt.status = "planned";
            receipt.removed = removed;
            return Ok(());
        }
        ensure_writable(&receipt.path)?;
        let _lock =
            lock_project_config(root).map_err(|e| format!("cannot lock config ({})", e.kind()))?;
        let before = read_config(&receipt.path)?.ok_or("config disappeared while locking")?;
        let (after, removed) = retirement_edit(&before)?;
        if removed.is_empty() {
            return Ok(());
        }
        ensure_writable(&receipt.path)?;
        let backup = backup_config(root, before.as_bytes())
            .map_err(|e| format!("cannot preserve config before-copy ({})", e.kind()))?;
        receipt.backup = Some(backup);
        atomic_replace_project_config(&receipt.path, &after)
            .and_then(|()| fs::File::open(root)?.sync_all())
            .map_err(|e| format!("cannot commit retired config cleanup ({})", e.kind()))?;
        receipt.removed = removed;
        receipt.status = "repaired";
        Ok(())
    })();
    if let Err(error) = result {
        receipt.status = "failed";
        receipt.error = Some(error);
    }
    receipt
}

fn read_config(path: &Path) -> Result<Option<String>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot inspect config ({})", e.kind())),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("config is not a regular owned file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err("config has multiple hard links; choose its canonical owner".into());
        }
    }
    fs::read_to_string(path)
        .map(Some)
        .map_err(|e| format!("cannot read config ({})", e.kind()))
}

// Reading and planning never require write permission. Check only when an
// actual retirement remains, both before lock creation and after the re-read.
fn ensure_writable(path: &Path) -> Result<(), String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|e| format!("cannot inspect config ({})", e.kind()))?;
    if metadata.permissions().readonly() {
        return Err("config is read-only; restore owner write permission before cleanup".into());
    }
    Ok(())
}

fn backup_config(root: &Path, before: &[u8]) -> std::io::Result<PathBuf> {
    let directory = root.join("config-retirement-backups");
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(&directory) {
        Ok(()) => fs::File::open(root)?.sync_all()?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let metadata = fs::symlink_metadata(&directory)?;
    let mut safe = metadata.is_dir() && !metadata.file_type().is_symlink();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        safe &= metadata.permissions().mode() & 0o077 == 0;
    }
    if !safe {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "unsafe backup directory",
        ));
    }
    let mut file = tempfile::Builder::new()
        .prefix("config-")
        .suffix(".toml")
        .tempfile_in(&directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(before)?;
    file.as_file().sync_all()?;
    let (_, path) = file.keep().map_err(|e| e.error)?;
    fs::File::open(&directory)?.sync_all()?;
    Ok(path)
}

/// Source-proven ignored fields only: factory's existing retirement registry,
/// Jev's removed integration, the never-wired TUI pop-up notification keys, and
/// code_review.owner (dispatch removal 849a49f38).
/// Parser spans remove tokens, retaining untouched bytes and parser-identified
/// comments. In particular a '#' inside an old secret string is never a comment.
fn retirement_edit(raw: &str) -> Result<(String, Vec<String>), String> {
    // Do not include TOML's diagnostic source excerpt: config may hold secrets.
    let config: Config = toml::from_str(raw).map_err(|_| {
        "invalid config TOML or supported value; repair config before updating".to_owned()
    })?;
    let doc = toml_edit::Document::parse(raw).map_err(|_| "invalid config TOML".to_owned())?;
    let mut targets: Vec<Vec<String>> = config
        .factory
        .as_ref()
        .map(|factory| {
            factory
                .retired_keys()
                .into_iter()
                .map(|key| vec!["factory".into(), key.into()])
                .collect()
        })
        .unwrap_or_default();
    if let Some(notifications) = &config.notifications {
        targets.extend(
            notifications
                .retired_keys()
                .into_iter()
                .map(|key| vec!["notifications".into(), key.into()]),
        );
    }
    if config.retired_jev.is_some() {
        targets.push(vec!["jev".into()]);
    }
    let mut expected: toml::Value = toml::from_str(raw).map_err(|_| "invalid config TOML")?;
    if expected
        .get("code_review")
        .and_then(|v| v.get("owner"))
        .is_some()
    {
        targets.push(vec!["code_review".into(), "owner".into()]);
    }
    if targets.is_empty() {
        return Ok((raw.into(), Vec::new()));
    }
    for path in &targets {
        let mut parent = &mut expected;
        for key in &path[..path.len() - 1] {
            parent = parent.get_mut(key).ok_or("missing retired config parent")?;
        }
        parent
            .as_table_mut()
            .ok_or("invalid retired config parent")?
            .remove(path.last().unwrap());
    }
    let mut edits = Vec::new();
    table_edits(doc.as_table(), &[], &targets, raw, &mut edits)?;
    let mut comments = Vec::new();
    item_comments(doc.as_item(), raw, &mut comments);
    trivia_comments(doc.trailing(), raw, &mut comments);
    edits.sort_by_key(|span| span.start);
    let mut after = String::new();
    let mut position = 0;
    for span in edits {
        if span.start < position {
            return Err("overlapping config retirement spans".into());
        }
        after.push_str(&raw[position..span.start]);
        // Preserve comments and line boundaries in removed multiline values.
        let mut at = span.start;
        while at < span.end {
            if let Some(comment) = comments.iter().find(|c| c.start == at) {
                let end = comment.end.min(span.end);
                after.push_str(&raw[at..end]);
                at = end;
            } else {
                if raw.as_bytes()[at] == b'\n' {
                    after.push('\n');
                }
                at += 1;
            }
        }
        position = span.end;
    }
    after.push_str(&raw[position..]);
    let mut actual: toml::Value = toml::from_str(&after).map_err(|_| {
        "cannot safely preserve config formatting; repair retired entries manually".to_owned()
    })?;
    // Dotted-only ancestors cease to exist when their last leaf is removed.
    for key in ["factory", "code_review", "notifications"] {
        for value in [&mut expected, &mut actual] {
            if value
                .get(key)
                .and_then(toml::Value::as_table)
                .is_some_and(|t| t.is_empty())
            {
                value.as_table_mut().unwrap().remove(key);
            }
        }
    }
    if actual != expected {
        return Err(
            "config retirement would change another value; repair retired entries manually".into(),
        );
    }
    Ok((after, targets.iter().map(|path| path.join(".")).collect()))
}

fn retired(path: &[String], targets: &[Vec<String>]) -> bool {
    targets.iter().any(|target| path.starts_with(target))
}

fn table_edits(
    table: &Table,
    base: &[String],
    targets: &[Vec<String>],
    raw: &str,
    edits: &mut Vec<Range<usize>>,
) -> Result<(), String> {
    if retired(base, targets) && !table.is_implicit() && !table.is_dotted() {
        edits.push(table.span().ok_or("missing retired table span")?);
    }
    for (keys, value) in table
        .get_values()
        .into_iter()
        .filter(|_| !table.is_dotted())
    {
        let path: Vec<_> = base
            .iter()
            .cloned()
            .chain(keys.iter().map(|key| key.get().to_owned()))
            .collect();
        if retired(&path, targets) {
            let span = value.span().ok_or("missing retired value span")?;
            // Dotted parents share a Key node (and its first spelling's
            // span). A TOML assignment always starts on the value's first
            // line, even when the value itself spans several lines.
            let start = raw[..span.start].rfind('\n').map_or(0, |at| at + 1);
            let indent = raw[start..span.start].len()
                - raw[start..span.start].trim_start_matches([' ', '\t']).len();
            edits.push(start + indent..span.end);
        } else if let Value::InlineTable(inline) = value {
            inline_edits(inline, &path, targets, raw, edits)?;
        }
    }
    for (key, item) in table.iter() {
        let mut path = base.to_vec();
        path.push(key.into());
        match item {
            Item::Table(child) => table_edits(child, &path, targets, raw, edits)?,
            Item::ArrayOfTables(array) => {
                for child in array.iter() {
                    table_edits(child, &path, targets, raw, edits)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn inline_edits(
    table: &toml_edit::InlineTable,
    base: &[String],
    targets: &[Vec<String>],
    raw: &str,
    edits: &mut Vec<Range<usize>>,
) -> Result<(), String> {
    let mut values = table.get_values();
    values.sort_by_key(|(_, value)| value.span().map(|span| span.start));
    let mut spans = Vec::new();
    let mut start = table.span().ok_or("missing inline table span")?.start + 1;
    for (keys, value) in &values {
        let path: Vec<_> = base
            .iter()
            .cloned()
            .chain(keys.iter().map(|key| key.get().to_owned()))
            .collect();
        let remove = retired(&path, targets);
        let value_span = value.span().ok_or("missing inline value span")?;
        // Between parsed values there is only the separator and trivia;
        // commas or equals signs inside quoted keys are never searched.
        let gap = &raw[start..value_span.start];
        let offset = gap.len() - gap.trim_start_matches([',', ' ', '\t', '\r', '\n']).len();
        spans.push((start + offset..value_span.end, remove));
        start = value_span.end;
        if !remove && let Value::InlineTable(child) = value {
            inline_edits(child, &path, targets, raw, edits)?;
        }
    }
    let mut i = 0;
    while i < spans.len() {
        if !spans[i].1 {
            i += 1;
            continue;
        }
        let first = i;
        while i < spans.len() && spans[i].1 {
            i += 1;
        }
        let span = if i < spans.len() {
            spans[first].0.start..spans[i].0.start
        } else if first > 0 {
            spans[first - 1].0.end..spans[i - 1].0.end
        } else {
            spans[first].0.start..spans[i - 1].0.end
        };
        edits.push(span);
    }
    Ok(())
}

fn trivia_comments(trivia: &RawString, raw: &str, comments: &mut Vec<Range<usize>>) {
    if let Some(span) = trivia.span() {
        let mut at = span.start;
        for line in raw[span.clone()].split_inclusive('\n') {
            if let Some(hash) = line.find('#') {
                comments.push(at + hash..at + line.trim_end_matches('\n').len());
            }
            at += line.len();
        }
    }
}

fn decor_comments(decor: &Decor, raw: &str, comments: &mut Vec<Range<usize>>) {
    for trivia in [decor.prefix(), decor.suffix()].into_iter().flatten() {
        trivia_comments(trivia, raw, comments);
    }
}

fn value_comments(value: &Value, raw: &str, comments: &mut Vec<Range<usize>>) {
    decor_comments(value.decor(), raw, comments);
    match value {
        Value::Array(array) => {
            trivia_comments(array.trailing(), raw, comments);
            for value in array.iter() {
                value_comments(value, raw, comments);
            }
        }
        Value::InlineTable(table) => {
            for (keys, value) in table.get_values() {
                for key in keys {
                    decor_comments(key.leaf_decor(), raw, comments);
                    decor_comments(key.dotted_decor(), raw, comments);
                }
                value_comments(value, raw, comments);
            }
        }
        _ => {}
    }
}

fn item_comments(item: &Item, raw: &str, comments: &mut Vec<Range<usize>>) {
    match item {
        Item::Table(table) => {
            decor_comments(table.decor(), raw, comments);
            for (key, item) in table.iter() {
                let key = table.key(key).unwrap();
                decor_comments(key.leaf_decor(), raw, comments);
                decor_comments(key.dotted_decor(), raw, comments);
                item_comments(item, raw, comments);
            }
        }
        Item::Value(value) => value_comments(value, raw, comments),
        Item::ArrayOfTables(array) => {
            for table in array.iter() {
                // Table traversal also gathers nested value decoration.
                item_comments(&Item::Table(table.clone()), raw, comments);
            }
        }
        Item::None => {}
    }
}
