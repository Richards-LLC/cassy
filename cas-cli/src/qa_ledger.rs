//! One ledger grammar for CLI preflight, close and verifier evidence gates.
//! This checks structure and capture readability, never the truth of a capture.
use std::{collections::{BTreeMap, BTreeSet}, path::Path};
use serde::Serialize;

pub const HEADER: &str = "| id | cell | expected | observed | verdict | label | evidence path | defect task |";
const LABELS: &[&str] = &["source-inferred", "fixture", "real-build", "eyewitness", "eyewitness/telemetry", "deployed-verification"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Verdict {
    #[serde(rename = "PASS")] Pass,
    #[serde(rename = "FAIL")] Fail,
    #[serde(rename = "NOT A ROUTE")] NotARoute,
    #[serde(rename = "NOT EXERCISED")] NotExercised,
    #[serde(rename = "DEFERRED")] Deferred,
}
impl Verdict {
    fn name(self) -> &'static str {
        match self { Self::Pass => "PASS", Self::Fail => "FAIL", Self::NotARoute => "NOT A ROUTE", Self::NotExercised => "NOT EXERCISED", Self::Deferred => "DEFERRED" }
    }
    fn parse(value: &str) -> Option<Self> {
        match value {
            "PASS" => Some(Self::Pass), "FAIL" => Some(Self::Fail), "NOT A ROUTE" => Some(Self::NotARoute),
            "NOT EXERCISED" => Some(Self::NotExercised), "DEFERRED" => Some(Self::Deferred),
            _ if value.strip_prefix("NOT EXERCISED:").is_some_and(|reason| !reason.trim().is_empty() && reason.trim() != "<reason>") => Some(Self::NotExercised),
            _ => None,
        }
    }
}
#[derive(Debug, Serialize)]
pub struct Row {
    pub line: usize,
    pub id: String,
    pub cell: String,
    pub expected: String,
    pub observed: String,
    pub verdict: Verdict,
    pub label: String,
    pub evidence_path: String,
    pub defect_task: String,
}
impl Row {
    pub fn deferred_owner(&self) -> Option<&str> {
        self.evidence_path.strip_prefix("deferred: deployed-verification owner=")
            .filter(|owner| !owner.is_empty() && !owner.chars().any(|c| c.is_whitespace() || c.is_control()))
    }
}
#[derive(Debug, Serialize)]
pub struct Diagnostic {
    pub line: usize,
    pub row_id: Option<String>,
    pub field: String,
    pub code: String,
    pub message: String,
    pub remedy: String,
}
#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub ok: bool,
    pub rows: Vec<Row>,
    pub counts: BTreeMap<String, usize>,
    pub labels: BTreeMap<String, usize>,
    pub diagnostics: Vec<Diagnostic>,
}
impl Report {
    fn error(&mut self, line: usize, id: Option<&str>, field: &str, code: &str, message: impl Into<String>, remedy: &str) {
        self.diagnostics.push(Diagnostic { line, row_id: id.map(str::to_owned), field: field.into(), code: code.into(), message: message.into(), remedy: remedy.into() });
    }
    pub fn refusal(&self) -> String {
        self.diagnostics.iter().map(|d| format!("line {}{} [{}]: {}; {}", d.line, d.row_id.as_ref().map(|id| format!(" row {id}")).unwrap_or_default(), d.field, d.message, d.remedy)).collect::<Vec<_>>().join("\n")
    }
}
fn cells(line: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut escaped = false;
    for c in line.trim().trim_matches('|').chars() {
        if c == '|' && !escaped { out.push(String::new()); }
        else if c == '\\' && !escaped { escaped = true; continue; }
        else { out.last_mut().unwrap().push(c); }
        escaped = false;
    }
    out.into_iter().map(|s| s.trim().trim_matches('`').trim().to_owned()).collect()
}

/// Parse without filesystem I/O. Journey coverage consumes these same typed rows.
pub fn parse(body: &str, min_rows: usize) -> Report {
    let mut report = Report::default();
    let mut header = false;
    let mut matrix = false;
    let mut fence: Option<char> = None;
    let mut metadata = String::new();
    let mut label_header: Option<(usize, String)> = None;
    let mut ids = BTreeSet::new();
    let expected_header = cells(HEADER);
    for (index, line) in body.lines().enumerate() {
        let n = index + 1;
        let line = line.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            let marker = line.chars().next().unwrap();
            if fence == Some(marker) { fence = None; } else if fence.is_none() { fence = Some(marker); }
            continue;
        }
        if fence.is_some() { continue; }
        if !header {
            if let Some(labels) = line.strip_prefix("labels:") { label_header = Some((n, labels.into())); }
            metadata.push_str(line); metadata.push('\n');
        }
        if !line.contains('|') { if matrix && line.starts_with('#') { matrix = false; } continue; }
        let c = cells(line);
        if c == expected_header {
            if header { report.error(n, None, "header", "duplicate_header", "duplicate matrix header", "keep one evidence matrix"); }
            header = true; matrix = true; continue;
        }
        if c.iter().all(|v| !v.is_empty() && v.chars().all(|ch| ch == '-' || ch == ':')) { continue; }
        if !matrix {
            // Do not silently accept a headerless matrix, or count later audit tables.
            if !header && c.len() == 8 { report.error(n, c.first().map(String::as_str), "header", "missing_header", "row appears before the canonical matrix header", "run cas qa ledger init --task <id>"); }
            continue;
        }
        if c.len() != 8 {
            report.error(n, c.first().map(String::as_str), "row", "columns", format!("expected 8 columns, observed {}", c.len()), "use the scaffold's eight columns; escape literal pipes with \\|"); continue;
        }
        let id = c[0].as_str();
        if id.is_empty() || !ids.insert(id.to_owned()) { report.error(n, Some(id), "id", "row_id", "row id is blank or duplicated", "give each matrix row a unique id"); }
        for field in [1, 2, 3] {
            if c[field].is_empty() { report.error(n, Some(id), ["id", "cell", "expected", "observed"][field], "required", "required cell is blank", "describe this cell and its observed result"); }
        }
        let Some(verdict) = Verdict::parse(&c[4]) else {
            report.error(n, Some(id), "verdict", "verdict", format!("invalid verdict {:?}; partial is not a verdict", c[4]), "use a scaffold verdict and record unrun work as NOT EXERCISED"); continue;
        };
        if !LABELS.contains(&c[5].as_str()) { report.error(n, Some(id), "label", "label", format!("invalid evidence label {:?}", c[5]), "use one evidence label from the scaffold"); }
        if verdict == Verdict::Pass && c[5] == "source-inferred" { report.error(n, Some(id), "label", "source_pass", "source-inferred cannot prove PASS", "capture the real outcome or mark NOT EXERCISED"); }
        if verdict == Verdict::Fail && !(c[7].starts_with("cas-") && c[7].len() > 4 && c[7].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')) {
            report.error(n, Some(id), "defect task", "defect_owner", "FAIL has no cas-* defect task", "file the defect and put its task id in this row");
        }
        let row = Row { line: n, id: id.into(), cell: c[1].clone(), expected: c[2].clone(), observed: c[3].clone(), verdict, label: c[5].clone(), evidence_path: c[6].clone(), defect_task: c[7].clone() };
        if (verdict == Verdict::Deferred || row.label == "deployed-verification") && (verdict != Verdict::Deferred || row.label != "deployed-verification" || row.deferred_owner().is_none()) {
            report.error(n, Some(id), "evidence path", "deferred_owner", "invalid deployed-verification deferral", "use DEFERRED / deployed-verification / deferred: deployed-verification owner=<registered supervisor>");
        }
        *report.counts.entry(verdict.name().into()).or_default() += 1;
        *report.labels.entry(row.label.clone()).or_default() += 1;
        report.rows.push(row);
    }
    if !header { report.error(1, None, "header", "missing_header", "canonical evidence matrix header is missing", "run cas qa ledger init --task <id>"); }
    report.counts.insert("cells".into(), report.rows.len());
    if report.rows.len() < min_rows { report.error(1, None, "rows", "breadth", format!("expected at least {min_rows} rows, observed {}", report.rows.len()), "include the demo happy path and three independent conditions"); }
    let count_re = regex::Regex::new(r"\b(cells|PASS|FAIL|NOT EXERCISED|NOT A ROUTE|DEFERRED)\s*[:=]\s*(\d+)").unwrap();
    let mut counts = BTreeMap::new();
    for c in count_re.captures_iter(&metadata) {
        if counts.insert(c[1].to_owned(), c[2].parse::<usize>().ok()).is_some() { report.error(1, None, &c[1], "duplicate_count", "duplicate headline count", "keep one count per verdict"); }
    }
    for name in ["cells", "PASS", "FAIL", "NOT EXERCISED", "NOT A ROUTE", "DEFERRED"] {
        let actual = report.counts.get(name).copied().unwrap_or(0);
        let required = ["cells", "PASS", "FAIL", "NOT EXERCISED"].contains(&name);
        if (required || counts.contains_key(name)) && counts.get(name).copied().flatten() != Some(actual) {
            report.error(1, None, name, "count", format!("headline {name} must equal {actual}"), "update the headline from the parsed row totals");
        }
    }
    let label_re = regex::Regex::new(r"([A-Za-z/-]+)\s*=\s*(\d+)").unwrap();
    let declared: BTreeMap<_, _> = label_header.as_ref().map(|(_, s)| label_re.captures_iter(s).map(|c| (c[1].to_owned(), c[2].parse::<usize>().unwrap_or(usize::MAX))).collect()).unwrap_or_default();
    if label_header.is_none() || declared != report.labels { report.error(label_header.map(|h| h.0).unwrap_or(1), None, "labels", "label_counts", format!("labels must equal {:?}", report.labels), "set labels: <label>=<count> for each used label"); }
    report.diagnostics.sort_by_key(|d| d.line);
    report.ok = report.diagnostics.is_empty();
    report
}

/// The same parser plus safe readable PASS captures, relative to the ledger.
pub fn check(path: &Path, min_rows: usize) -> Report {
    let body = match std::fs::read_to_string(path) {
        Ok(body) => body,
        Err(error) => { let mut report = Report::default(); report.error(1, None, "path", "read", format!("cannot read ledger: {error}"), "create the ledger with cas qa ledger init --task <id>"); return report; }
    };
    let mut report = parse(&body, min_rows);
    let root = path.parent().unwrap_or(Path::new(".")).canonicalize();
    let mut failures = Vec::new();
    for row in report.rows.iter().filter(|r| r.verdict == Verdict::Pass) {
        let readable = root.as_ref().ok().and_then(|root| {
            let relative = Path::new(&row.evidence_path);
            if relative.is_absolute() || relative.components().any(|p| matches!(p, std::path::Component::ParentDir)) { return None; }
            crate::qa_evidence::resolve_bundle_file(root, "ledger capture", &row.evidence_path).ok()
        }).is_some_and(|p| p.is_file() && std::fs::File::open(p).is_ok());
        if !readable { failures.push((row.line, row.id.clone())); }
    }
    for (line, id) in failures { report.error(line, Some(&id), "evidence path", "capture", "PASS capture is missing, unreadable or outside the ledger directory", "save a readable capture inside the task evidence directory"); }
    report.diagnostics.sort_by_key(|d| d.line);
    report.ok = report.diagnostics.is_empty();
    report
}

pub fn minimum_rows(task: &cas_types::Task) -> usize {
    if task.execution_note.as_deref() != Some("no-code") && !task.demo_statement.trim().is_empty() { 4 } else { 1 }
}

/// Honest starting point. Predicted journeys retain their IDs for G8 coverage.
pub fn scaffold(task: &cas_types::Task, journeys: &[(String, String)], revision: &str) -> String {
    let no_code = task.execution_note.as_deref() == Some("no-code");
    let mut rows: Vec<_> = journeys.iter().map(|(id, name)| (id.clone(), name.clone())).collect();
    let min = minimum_rows(task);
    if rows.is_empty() { rows.push(("M01".into(), if task.demo_statement.is_empty() { "Review delivery evidence".into() } else { task.demo_statement.clone() })); }
    for name in ["Invalid input", "Boundary condition", "Adjacent surface"] {
        if rows.len() < min { rows.push((format!("M{:02}", rows.len() + 1), name.into())); }
    }
    let verdict = if no_code { "NOT A ROUTE" } else { "NOT EXERCISED" };
    let clean = |s: &str| s.replace(['\n', '\r'], " ").replace('|', "\\|");
    let mut out = format!("# QA evidence: {}\n\nbuild: {}\nscope: {}\nsurface: {}\nbudget: 30 minutes\ncells={}; PASS=0; FAIL=0; NOT EXERCISED={}; NOT A ROUTE={}\nlabels: source-inferred={}\nsweep: not configured\n\n{}\n| --- | --- | --- | --- | --- | --- | --- | --- |\n", task.id, clean(revision), clean(&task.title), if no_code { "no-code delivery" } else { "fill in the exercised surface" }, rows.len(), if no_code {0} else {rows.len()}, if no_code {rows.len()} else {0}, rows.len(), HEADER);
    for (id, name) in rows {
        out.push_str(&format!("| {} | {} | Describe the expected outcome | {} | {verdict} | source-inferred | - | - |\n", clean(&id), clean(&name), if no_code { "No runtime route; review the delivery artifact" } else { "Not run; capture against the named build" }));
    }
    out.push_str("\n## Constants vs expectation\n\n| constant | location | visible contract | predictable? | defect task |\n| --- | --- | --- | --- | --- |\n\n## Contradictions\n\n| terminal state | surface | visible claim | conflicting surface | defect task |\n| --- | --- | --- | --- | --- |\n\n## Honesty\n\nRecord unavailable devices, automation artifacts and unrun conditions here.\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaffold_round_trip_preserves_honest_rows_and_task_policy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("LEDGER.md");
        let mut task = cas_types::Task::new("cas-ledger".into(), "CLI | evidence".into());
        task.demo_statement = "Open command help".into();
        let journeys = vec![("CLI-J1".into(), "Help | reader".into())];
        for no_code in [false, true] {
            task.execution_note = no_code.then(|| "no-code".into());
            std::fs::write(&path, scaffold(&task, &journeys, "fixture-revision")).unwrap();
            let report = crate::cli::qa_ledger::check(&path, Some(&task));
            assert!(report.ok, "{}", report.refusal());
            assert_eq!(report.rows[0].id, "CLI-J1");
            assert_eq!(report.rows[0].cell, "Help | reader");
            assert_eq!(report.rows.len(), if no_code { 1 } else { 4 });
            assert!(report.rows.iter().all(|r| r.verdict == if no_code { Verdict::NotARoute } else { Verdict::NotExercised }));
        }
    }

    #[test]
    fn diagnostics_reject_bad_counts_labels_verdicts_and_escaping_captures() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("LEDGER.md");
        let base = format!("cells=1; PASS=1; FAIL=0; NOT EXERCISED=0\nlabels: real-build=1\n{HEADER}\n| M01 | help | usage | usage | PASS | real-build | capture.txt | - |\n");
        std::fs::write(dir.path().join("capture.txt"), "usage").unwrap();
        std::fs::write(&path, &base).unwrap();
        assert!(check(&path, 1).ok);
        for (body, code) in [
            (base.replace("cells=1", "cells=2"), "count"),
            (base.replace("labels: real-build=1", "labels: real-build=2"), "label_counts"),
            (base.replace("| PASS |", "| partial |"), "verdict"),
            (base.replace("real-build", "source-inferred"), "source_pass"),
            (base.replace("| real-build |", "| real-build/fixture |"), "label"),
            (base.replace("capture.txt", "../outside.txt"), "capture"),
            (base.replace("capture.txt", "missing.txt"), "capture"),
        ] {
            std::fs::write(&path, body).unwrap();
            let report = check(&path, 1);
            assert!(!report.ok && report.diagnostics.iter().any(|d| d.code == code), "{code}: {}", report.refusal());
        }
        // A fenced example is not proof; other audit tables do not count.
        assert_eq!(parse(&format!("```\n{base}```\n"), 1).rows.len(), 0);
    }
}
