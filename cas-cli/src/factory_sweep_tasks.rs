//! Failure-class reports emitted by the factory's integration sweep.
//!
//! The daemon owns report generation, while the supervisor-facing MCP handler
//! consumes the same durable JSON.  Keeping the parser and wire format here
//! prevents those two paths from growing subtly different ideas of what a
//! "failure class" is.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};

const MERGE_SWEEP_DIR: &str = "merge-sweeps";
pub(crate) const SWEEP_TASKS_FILE: &str = "sweep-tasks.json";
const MAX_ASSERTION_CHARS: usize = 1_600;

/// One ready-to-file task, before a supervisor accepts the report.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct SweepTaskProposal {
    pub failure_class: String,
    pub class_name: String,
    pub title: String,
    pub failing_tests: Vec<String>,
    #[serde(default)]
    pub failing_targets: Vec<String>,
    pub assertion_text: String,
    pub log_path: String,
    pub suggested_lane: String,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub spawn_request_id: Option<String>,
    #[serde(default)]
    pub spawn_error: Option<String>,
}

/// Durable output of one integration sweep.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct SweepTaskReport {
    pub schema_version: u32,
    pub status: String,
    pub generated_at: String,
    pub integration_branch: String,
    pub integration_tip: String,
    pub source_epic: String,
    #[serde(default)]
    pub affected_epics: Vec<String>,
    pub log_path: String,
    pub failure_count: usize,
    pub classes: Vec<SweepTaskProposal>,
    #[serde(default)]
    pub accepted_at: Option<String>,
}

#[derive(Debug, Clone)]
struct FailureRecord {
    binary: String,
    test_name: String,
    target: String,
    assertion_text: String,
}

#[derive(Debug, Clone)]
struct FailureClass {
    id: String,
    name: String,
    title: String,
    lane: String,
}

pub(crate) fn report_path(cas_dir: &Path) -> PathBuf {
    cas_dir.join(MERGE_SWEEP_DIR).join(SWEEP_TASKS_FILE)
}

pub(crate) fn read_report(
    cas_dir: &Path,
    requested: Option<&str>,
) -> Result<SweepTaskReport, String> {
    let path = requested
        .map(PathBuf::from)
        .unwrap_or_else(|| report_path(cas_dir));
    let bytes = fs::read(&path)
        .map_err(|error| format!("cannot read sweep task report {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("cannot parse sweep task report {}: {error}", path.display()))
}

pub(crate) fn write_report(cas_dir: &Path, report: &SweepTaskReport) -> Result<PathBuf, String> {
    let path = report_path(cas_dir);
    fs::create_dir_all(path.parent().ok_or("sweep task report parent missing")?)
        .map_err(|error| format!("create sweep task report directory: {error}"))?;
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(report)
        .map_err(|error| format!("encode sweep task report: {error}"))?;
    fs::write(&temporary, bytes)
        .map_err(|error| format!("write sweep task report temporary file: {error}"))?;
    fs::rename(&temporary, &path).map_err(|error| format!("publish sweep task report: {error}"))?;
    Ok(path)
}

/// Parse one sweep log into one proposal per failure class.
pub(crate) fn build_report(
    project_root: &Path,
    log_path: &Path,
    status: &str,
    integration_branch: &str,
    integration_tip: &str,
    source_epic: &str,
    affected_epics: &[String],
) -> Result<SweepTaskReport, String> {
    let contents = fs::read_to_string(log_path)
        .map_err(|error| format!("cannot read sweep log {}: {error}", log_path.display()))?;
    Ok(build_report_from_contents(
        project_root,
        log_path,
        status,
        integration_branch,
        integration_tip,
        source_epic,
        affected_epics,
        &contents,
    ))
}

fn build_report_from_contents(
    project_root: &Path,
    log_path: &Path,
    status: &str,
    integration_branch: &str,
    integration_tip: &str,
    source_epic: &str,
    affected_epics: &[String],
    contents: &str,
) -> SweepTaskReport {
    let failures = parse_failures(contents);
    let mut grouped: BTreeMap<String, (FailureClass, Vec<FailureRecord>)> = BTreeMap::new();
    for failure in failures.iter().cloned() {
        let class = classify_failure(project_root, &failure);
        grouped
            .entry(class.id.clone())
            .or_insert_with(|| (class, Vec::new()))
            .1
            .push(failure);
    }

    let classes = grouped
        .into_values()
        .map(|(class, records)| {
            let mut tests = Vec::new();
            let mut targets = Vec::new();
            let mut assertions = Vec::new();
            for record in records {
                if !tests.contains(&record.test_name) {
                    tests.push(record.test_name);
                }
                if !targets.contains(&record.target) {
                    targets.push(record.target);
                }
                if !record.assertion_text.is_empty() && !assertions.contains(&record.assertion_text)
                {
                    assertions.push(record.assertion_text);
                }
            }
            let assertion_text = truncate_assertions(&assertions.join("\n"));
            SweepTaskProposal {
                failure_class: class.id,
                class_name: class.name,
                title: class.title,
                failing_tests: tests,
                failing_targets: targets,
                assertion_text,
                log_path: log_path.display().to_string(),
                suggested_lane: class.lane,
                task_id: None,
                spawn_request_id: None,
                spawn_error: None,
            }
        })
        .collect();

    SweepTaskReport {
        schema_version: 1,
        status: status.to_owned(),
        generated_at: Utc::now().to_rfc3339(),
        integration_branch: integration_branch.to_owned(),
        integration_tip: integration_tip.to_owned(),
        source_epic: source_epic.to_owned(),
        affected_epics: affected_epics.to_vec(),
        log_path: log_path.display().to_string(),
        failure_count: failures.len(),
        classes,
        accepted_at: None,
    }
}

fn parse_failures(contents: &str) -> Vec<FailureRecord> {
    let lines: Vec<&str> = contents.lines().collect();
    let starts: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| parse_failure_header(line).map(|_| index))
        .collect();
    starts
        .iter()
        .enumerate()
        .filter_map(|(position, start)| {
            let end = starts.get(position + 1).copied().unwrap_or(lines.len());
            let (binary, test_name) = parse_failure_header(lines[*start])?;
            let assertion_text = assertion_excerpt(&lines[*start..end]);
            Some(FailureRecord {
                // Preserve nextest's executable/test target shape. This is
                // also the shape accepted by the existing failing-target
                // filter, so a ready-to-file task can be copied directly into
                // a focused rerun command.
                target: format!("{binary} {test_name}"),
                binary,
                test_name,
                assertion_text,
            })
        })
        .collect()
}

fn parse_failure_header(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    if !trimmed.starts_with("FAIL [") {
        return None;
    }
    let (_, rest) = trimmed.split_once(']')?;
    let mut fields = rest.split_whitespace();
    let mut binary = fields.next()?;
    if binary.starts_with('(') {
        binary = fields.next()?;
    }
    let test_name = fields.collect::<Vec<_>>().join(" ");
    if test_name.is_empty() {
        return None;
    }
    Some((binary.to_owned(), test_name))
}

fn assertion_excerpt(lines: &[&str]) -> String {
    let panic_index = lines.iter().position(|line| {
        let lower = line.to_ascii_lowercase();
        lower.contains("panicked at") || lower.contains("assertion")
    });
    let Some(panic_index) = panic_index else {
        return String::new();
    };
    let mut excerpt = Vec::new();
    for line in lines.iter().skip(panic_index + 1) {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("note:")
            || trimmed.starts_with("left:")
            || trimmed.starts_with("right:")
            || trimmed == "stdout ───"
            || trimmed == "stderr ───"
        {
            continue;
        }
        if trimmed.starts_with("Summary ") || trimmed.starts_with("FAIL [") {
            break;
        }
        excerpt.push(trimmed);
        // Keep the assertion message and the first few diagnostic lines, but
        // never let a verbose test body turn the task receipt into a log copy.
        if excerpt.len() == 4 {
            break;
        }
    }
    if !excerpt.is_empty() {
        return truncate_assertions(&excerpt.join("\n"));
    }
    truncate_assertions(lines[panic_index].trim())
}

fn truncate_assertions(value: &str) -> String {
    if value.chars().count() <= MAX_ASSERTION_CHARS {
        return value.to_owned();
    }
    let mut truncated: String = value.chars().take(MAX_ASSERTION_CHARS - 3).collect();
    truncated.push_str("...");
    truncated
}

fn classify_failure(project_root: &Path, failure: &FailureRecord) -> FailureClass {
    let haystack = format!(
        "{} {} {}",
        failure.binary.to_ascii_lowercase(),
        failure.test_name.to_ascii_lowercase(),
        failure.assertion_text.to_ascii_lowercase()
    );

    // These are the two classes in the 3.25.0 assembly incident. The archive
    // portability assertion was added by the same liveness delivery and must
    // travel with that fix instead of becoming a third serial round.
    if haystack.contains("worker_status") || haystack.contains("builtin_archive_portability_test") {
        return FailureClass {
            id: "cas-7b7b".to_owned(),
            name: "worker_status contract".to_owned(),
            title: "Fix cas-7b7b worker_status contract failures".to_owned(),
            lane: "standard".to_owned(),
        };
    }
    if haystack.contains("task_update_work_target_close")
        || haystack.contains("declared target branch")
        || haystack.contains("pre-close hook context rejected")
    {
        return FailureClass {
            id: "cas-f7c8".to_owned(),
            name: "wording pin".to_owned(),
            title: "Fix cas-f7c8 wording pin failures".to_owned(),
            lane: "light".to_owned(),
        };
    }

    if let Some(label) = known_failure_log_class(project_root, &haystack) {
        let lane = if label.contains("manual") {
            "light"
        } else {
            "standard"
        };
        return FailureClass {
            id: format!("known:{label}"),
            name: label.clone(),
            title: format!("Fix {label} sweep failures"),
            lane: lane.to_owned(),
        };
    }

    FailureClass {
        id: "unclassified".to_owned(),
        name: "unclassified".to_owned(),
        title: "Investigate unclassified sweep failures".to_owned(),
        lane: "standard".to_owned(),
    }
}

fn known_failure_log_class(project_root: &Path, haystack: &str) -> Option<String> {
    let path =
        project_root.join("cas-cli/src/builtins/skills/cas-cut-release/references/failure-log.md");
    let contents = fs::read_to_string(path).ok()?;
    let mut best: Option<(usize, String)> = None;
    for line in contents.lines() {
        let Some(start) = line.find("**") else {
            continue;
        };
        let rest = &line[start + 2..];
        let Some(end) = rest.find("**") else { continue };
        let label = rest[..end].trim();
        if label.is_empty() {
            continue;
        }
        let terms = line
            .split(|character: char| !character.is_ascii_alphanumeric())
            .map(str::to_ascii_lowercase)
            .filter(|term| term.len() >= 5 && !is_stop_term(term))
            .collect::<Vec<_>>();
        let score = terms
            .iter()
            .filter(|term| haystack.contains(term.as_str()))
            .count();
        if score >= 2 && best.as_ref().is_none_or(|(known, _)| score > *known) {
            best = Some((score, label.to_owned()));
        }
    }
    best.map(|(_, label)| label)
}

fn is_stop_term(term: &str) -> bool {
    matches!(
        term,
        "symptom" | "root" | "cause" | "release" | "operator" | "reported" | "failed" | "failure"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actual_assembly_failures_collapse_into_two_fix_classes() {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("sweep.log");
        let contents = r#"
     Summary [  24.878s] 9313 tests run: 9305 passed, 8 failed, 90 skipped
        FAIL [   0.172s] (5762/9313) cas::builtin_archive_portability_test builtin_inspection_tests_do_not_depend_on_the_checkout_at_runtime
    thread 'builtin_inspection_tests_do_not_depend_on_the_checkout_at_runtime' panicked at cas-cli/tests/builtin_archive_portability_test.rs:756:5:
      cas-cli/tests/builtin_flavor_drift_test.rs:1559: env!("CARGO_MANIFEST_DIR")
        FAIL [   0.227s] (5950/9313) cas::factory_mcp_ops_test test_9829_worker_status_marks_stalled_worker_with_in_progress_task
    thread 'test_9829_worker_status_marks_stalled_worker_with_in_progress_task' panicked at cas-cli/tests/factory_mcp_ops_test.rs:3216:10:
    busy-badger row must be present
        FAIL [   0.190s] (6040/9313) cas::factory_mcp_ops_test test_worker_status_dedupes_nested_identity_with_missing_worktree
    thread 'test_worker_status_dedupes_nested_identity_with_missing_worktree' panicked at cas-cli/tests/factory_mcp_ops_test.rs:2307:5:
    assertion `left == right` failed: Worker Status
        FAIL [   0.154s] (6045/9313) cas::factory_mcp_ops_test test_worker_status_reports_between_turns_not_stalled_for_claude_worker
    thread 'test_worker_status_reports_between_turns_not_stalled_for_claude_worker' panicked at cas-cli/tests/factory_mcp_ops_test.rs:2788:5:
    the row must state the between-turns reality
        FAIL [   0.183s] (6085/9313) cas::factory_mcp_ops_test test_worker_status_stale_lease_alone_does_not_assert_work_in_progress
    thread 'test_worker_status_stale_lease_alone_does_not_assert_work_in_progress' panicked at cas-cli/tests/factory_mcp_ops_test.rs:2439:10:
    badger row
        FAIL [   0.221s] (6087/9313) cas::factory_mcp_ops_test test_worker_status_shows_inbox_depth_even_when_not_stalled
    thread 'test_worker_status_shows_inbox_depth_even_when_not_stalled' panicked at cas-cli/tests/factory_mcp_ops_test.rs:2637:10:
    wolf row
        FAIL [   0.443s] (6093/9313) cas::factory_mcp_ops_test test_worker_status_and_agent_list_agree_on_live_workers
    thread 'test_worker_status_and_agent_list_agree_on_live_workers' panicked at cas-cli/tests/factory_mcp_ops_test.rs:2951:5:
    dead worker must not be in worker_status Active roster
        FAIL [   0.443s] (6913/9313) cas::task_update_work_target_close_test combined_work_target_update_and_close_uses_the_updated_branch
    thread 'combined_work_target_update_and_close_uses_the_updated_branch' panicked at cas-cli/tests/task_update_work_target_close_test.rs:195:5:
    the worker commit is not merged into the updated alternate target; got:
    PRE-CLOSE HOOK CONTEXT REJECTED: live target_branch `alternate`
"#;
        fs::write(&log, contents).unwrap();
        let report = build_report_from_contents(
            temp.path(),
            &log,
            "FAILED",
            "integration/cas-src",
            "tip",
            "cas-c280",
            &["cas-c280".to_owned()],
            contents,
        );
        assert_eq!(report.failure_count, 8);
        assert_eq!(report.classes.len(), 2);
        assert_eq!(report.classes[0].failure_class, "cas-7b7b");
        assert_eq!(report.classes[0].failing_tests.len(), 7);
        assert!(report.classes[0].assertion_text.contains("busy-badger"));
        assert_eq!(report.classes[1].failure_class, "cas-f7c8");
        assert_eq!(
            report.classes[1].failing_tests,
            vec!["combined_work_target_update_and_close_uses_the_updated_branch"]
        );
        assert!(report.classes[1].assertion_text.contains("target_branch"));
    }

    #[test]
    fn parser_keeps_binary_and_test_target_separate() {
        let failures = parse_failures(
            "FAIL [0.1s] (1/2) cas::one_test alpha\nthread 'alpha' panicked at x:1:1:\nexpected alpha\nFAIL [0.2s] cas::two_test beta\nthread 'beta' panicked at x:2:1:\nexpected beta\n",
        );
        assert_eq!(failures[0].binary, "cas::one_test");
        assert_eq!(failures[0].test_name, "alpha");
        assert_eq!(failures[0].target, "cas::one_test alpha");
        assert_eq!(failures[1].assertion_text, "expected beta");
    }

    #[test]
    fn report_round_trips_at_the_durable_merge_sweep_path() {
        let temp = tempfile::tempdir().unwrap();
        let report = build_report_from_contents(
            temp.path(),
            Path::new("/artifacts/sweep.log"),
            "FAILED",
            "integration/cas-src",
            "tip",
            "cas-c280",
            &["cas-c280".to_owned()],
            "FAIL [0.1s] cas::factory_mcp_ops_test target\nthread 'target' panicked at x:1:1:\nassertion text\n",
        );
        let path = write_report(temp.path(), &report).unwrap();
        assert_eq!(path, report_path(temp.path()));
        assert_eq!(read_report(temp.path(), None).unwrap(), report);
    }

    #[test]
    fn unknown_assertion_is_unclassified() {
        let temp = tempfile::tempdir().unwrap();
        let report = build_report_from_contents(
            temp.path(),
            Path::new("/artifacts/sweep.log"),
            "FAILED",
            "integration/cas-src",
            "tip",
            "cas-848",
            &["cas-848".to_owned()],
            "FAIL [0.1s] cas::fixture novel_test\nthread 'novel_test' panicked at x:1:1:\nAn assertion never seen before\n",
        );
        assert_eq!(report.classes.len(), 1);
        assert_eq!(report.classes[0].failure_class, "unclassified");
        assert_eq!(report.classes[0].class_name, "unclassified");
    }
}
