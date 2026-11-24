//! GH #2401 item 4: park requires the worker's `AC-CHECKLIST.md` in the task
//! artifacts, with one row per acceptance criterion (`AC<n>`) and per
//! supervisor decision note (`D<n>`). A row is `MET` with a `file:line` and a
//! receipt, or `NOT MET`/`NARROWED` with a reason that a supervisor decision
//! note naming the item approves. Narrowing is asked, never recorded silently.
use cas_types::{EventEntityType, EventType, Task};
use regex::Regex;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::LazyLock;

pub(super) const FILE: &str = "AC-CHECKLIST.md";

static NUMBERED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)(?:^|\s)\(?(\d{1,2})[.)]\s").unwrap());
static ITEM_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(?:AC|D)\d+\b").unwrap());
static FILE_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\w./-]*[A-Za-z./][\w./-]*:\d+").unwrap());

/// Acceptance criteria in order: `1) a 2) b` or `1. a` lines, else bullet
/// lines, else the whole text as one criterion.
fn criteria(text: &str) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    let mut marks = Vec::new();
    for found in NUMBERED.captures_iter(text) {
        if found[1].parse::<usize>().ok() == Some(marks.len() + 1) {
            let marker = found.get(0).unwrap();
            marks.push((marker.start(), marker.end()));
        }
    }
    if marks.len() > 1 {
        return marks.iter().enumerate().map(|(index, &(_, body))| {
            let end = marks.get(index + 1).map_or(text.len(), |next| next.0);
            text[body..end].trim().to_string()
        }).collect();
    }
    let bullets: Vec<_> = text.lines().filter_map(|line| {
        line.trim_start().strip_prefix("- ").or_else(|| line.trim_start().strip_prefix("* "))
    }).map(|line| line.trim().to_string()).collect();
    if bullets.len() > 1 { bullets } else { vec![text.to_string()] }
}

/// Decision notes a live supervisor recorded on this task, oldest first. The
/// notes tool stamps their body on the event, so a worker cannot author one.
fn supervisor_decisions(cas_root: &Path, task_id: &str) -> Vec<String> {
    let Ok(events) = crate::store::open_event_store(cas_root)
        .and_then(|store| Ok(store.list_for_entity(EventEntityType::Task, task_id, 10_000)?))
    else {
        return Vec::new();
    };
    let mut decisions: Vec<_> = events.into_iter()
        .filter(|event| event.event_type == EventType::TaskNoteAdded)
        .filter_map(|event| {
            let note = event.metadata.as_ref()?.get("supervisor_decision")?.as_str()?.to_string();
            Some((event.id, note))
        })
        .collect();
    decisions.sort();
    decisions.into_iter().map(|(_, note)| note).collect()
}

fn names(text: &str, id: &str) -> bool {
    ITEM_ID.find_iter(text).any(|found| found.as_str().eq_ignore_ascii_case(id))
}

fn snippet(text: &str) -> String {
    crate::mcp::tools::truncate_str(&text.split_whitespace().collect::<Vec<_>>().join(" "), 80)
}

/// `(id, what it covers)` for every row the checklist must hold. A decision
/// naming an existing item approves that item and needs no row of its own.
fn items(task: &Task, decisions: &[String]) -> Vec<(String, String)> {
    let mut items: Vec<_> = criteria(&task.acceptance_criteria).into_iter().enumerate()
        .map(|(index, text)| (format!("AC{}", index + 1), text)).collect();
    for note in decisions {
        if !items.iter().any(|(id, _)| names(note, id)) {
            let id = format!("D{}", items.iter().filter(|(id, _)| id.starts_with('D')).count() + 1);
            items.push((id, format!("supervisor decision: {note}")));
        }
    }
    items
}

fn checklist_path(cas_root: &Path, task_id: &str) -> Result<std::path::PathBuf, String> {
    let config = crate::config::Config::load(cas_root).map_err(|error| error.to_string())?;
    let paths = crate::config::resolved_factory_artifact_paths(cas_root, config.factory().artifacts_root.as_deref());
    let [scoped, _] = paths.task_dirs(task_id);
    Ok(scoped.join(FILE))
}

/// `ID -> (STATUS, evidence or reason, receipt)`; the first row per item wins.
fn rows(body: &str) -> BTreeMap<String, (String, String, String)> {
    let mut rows = BTreeMap::new();
    for line in body.lines().map(str::trim).filter(|line| line.starts_with('|')) {
        let cells: Vec<_> = line.trim_matches('|').split('|').map(str::trim).collect();
        let id = cells[0].to_ascii_uppercase();
        if ITEM_ID.find(&id).is_none_or(|found| found.as_str() != id) {
            continue;
        }
        let cell = |index: usize| cells.get(index).copied().unwrap_or_default().to_string();
        let status = cell(1).split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_uppercase();
        rows.entry(id).or_insert((status, cell(2), cell(3)));
    }
    rows
}

/// The start-context line naming the rows park will require.
pub(super) fn requirement(cas_root: &Path, task: &Task) -> Option<String> {
    let items = items(task, &supervisor_decisions(cas_root, &task.id));
    let path = checklist_path(cas_root, &task.id).ok()?;
    (!items.is_empty()).then(|| format!(
        "AC checklist: `{}` needs a row per item ({}): MET + file:line + receipt, or NOT MET/NARROWED + reason approved by a supervisor decision note naming the item.",
        path.display(),
        items.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>().join(", "),
    ))
}

// gate[G-166 park as-of]: Park requires an AC checklist and refuses an unapproved narrowing
pub(super) fn at_park(cas_root: &Path, task: &Task) -> Result<(), String> {
    if std::env::var_os("CAS_FACTORY_MODE").is_none() {
        return Ok(());
    }
    let decisions = supervisor_decisions(cas_root, &task.id);
    let items = items(task, &decisions);
    if items.is_empty() {
        return Ok(());
    }
    let path = checklist_path(cas_root, &task.id)
        .map_err(|error| crate::gates::tag("G-166", format!("⚠️ AC CHECKLIST REQUIRED: task artifacts unresolved: {error}")))?;
    let body = std::fs::read_to_string(&path).ok();
    let rows = body.as_deref().map(rows).unwrap_or_default();
    let mut problems = Vec::new();
    let mut template = Vec::new();
    for (id, covers) in &items {
        let problem = match rows.get(id) {
            None => {
                template.push(format!("| {id} | MET | <path:line> | <receipt> |"));
                "no row".to_string()
            }
            Some((status, evidence, receipt)) if status == "MET" => {
                if FILE_LINE.is_match(evidence) && !receipt.is_empty() && !receipt.starts_with('<') {
                    continue;
                }
                "MET needs a file:line and a receipt".to_string()
            }
            Some((status, reason, _)) if status == "NOT MET" || status == "NARROWED" => {
                if reason.is_empty() || reason.starts_with('<') {
                    format!("{status} needs a reason")
                } else if decisions.iter().any(|note| names(note, id)) {
                    continue;
                } else {
                    format!("{status} has no supervisor decision note naming {id}; ask the supervisor instead of narrowing")
                }
            }
            Some((status, ..)) => format!("status `{status}` is not MET, NOT MET or NARROWED"),
        };
        problems.push(format!("- {id} ({}): {problem}", snippet(covers)));
    }
    if problems.is_empty() {
        return Ok(());
    }
    let mut message = format!(
        "⚠️ AC CHECKLIST REQUIRED (GH #2401): task {} parks only when `{}` accounts for every acceptance criterion and supervisor decision. {} of {} items fail:\n{}",
        task.id, path.display(), problems.len(), items.len(), problems.join("\n"),
    );
    if !template.is_empty() {
        if body.is_none() {
            template.splice(0..0, ["| Item | Status | Evidence or reason | Receipt |".into(), "| --- | --- | --- | --- |".into()]);
        }
        message.push_str(&format!("\n\nAdd these rows:\n{}", template.join("\n")));
    }
    message.push_str(&format!(
        "\n\nNext: fix the checklist, then retry `task action=close id={}`. A NOT MET or NARROWED row parks only after a supervisor records `task action=notes id={} note_type=decision notes=\"<item id> narrowed: <reason>\"`.",
        task.id, task.id,
    ));
    Err(crate::gates::tag("G-166", message))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn criteria_split_numbered_bulleted_and_plain_text() {
        for (text, count) in [
            ("1) Park refuses. 2) Park accepts v4.0. 3) Table test.", 3),
            ("1. one\n2. two", 2),
            ("- one\n- two\n- three", 3),
            ("Ship it in 2. parts", 1),
            ("", 0),
        ] {
            assert_eq!(criteria(text).len(), count, "{text:?}: {:?}", criteria(text));
        }
    }
}
