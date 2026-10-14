//! Operation-scoped lesson recall (cas-9734).
//!
//! Ambient recall matches the words of a turn, so the lesson that matters at
//! a release cut ("put the scratch base on the checkout's filesystem") lost to
//! whatever shared vocabulary with the prompt. Here the operation, not the
//! wording, selects the lessons: a memory tagged `op:<operation>` is surfaced
//! as a short checklist when a tool call starts that operation.
//!
//! The classification is deliberately literal and harness-neutral: a Skill
//! load, a release-train `--cut`, a factory `spawn_workers`, a task close of an
//! epic, or a merge command. Surfacing is rate-limited per session and
//! operation so a supervisor merging ten lanes reads the list once, not ten
//! times.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::hooks::HookInput;
use crate::types::Entry;

/// The operations a lesson can be tagged with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Operation {
    ReleaseCut,
    Spawn,
    Report,
    EpicClose,
    Merge,
}

impl Operation {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Operation::ReleaseCut => "release-cut",
            Operation::Spawn => "spawn",
            Operation::Report => "report",
            Operation::EpicClose => "epic-close",
            Operation::Merge => "merge",
        }
    }

    /// The memory tag that binds a lesson to this operation.
    pub(crate) fn tag(self) -> String {
        format!("op:{}", self.name())
    }
}

const MAX_LESSONS: usize = 6;
const MAX_LESSON_CHARS: usize = 180;
const RESURFACE_AFTER: Duration = Duration::from_secs(30 * 60);
const STATE_DIR: &str = "operation_lessons";

/// Classify a tool call as the start of an operation, if it is one.
///
/// `epic` answers whether a task id names an epic; it is only consulted for a
/// task close, so ordinary calls never open the task store.
pub(crate) fn classify(
    input: &HookInput,
    tool_prefix: &str,
    epic: impl FnOnce(&str) -> bool,
) -> Option<Operation> {
    let tool = input.tool_name.as_deref()?;
    let tool_input = input.tool_input.as_ref();
    let field = |name: &str| {
        tool_input
            .and_then(|value| value.get(name))
            .and_then(|value| value.as_str())
    };
    match tool {
        "Skill" => {
            let skill = field("skill")?;
            let skill = skill.rsplit(':').next().unwrap_or(skill);
            match skill {
                "cas-cut-release" => Some(Operation::ReleaseCut),
                "cas-release-report" | "cas-html-reports" => Some(Operation::Report),
                _ => None,
            }
        }
        "Bash" | "run_terminal_command" => classify_command(field("command")?),
        _ => {
            let action = field("action");
            let named = |suffix: &str| tool == format!("{tool_prefix}{suffix}");
            if (named("factory") || named("coordination"))
                && matches!(action, Some("spawn_workers" | "spawn_worker"))
            {
                return Some(Operation::Spawn);
            }
            if named("task") && action == Some("close") && field("id").is_some_and(epic) {
                return Some(Operation::EpicClose);
            }
            None
        }
    }
}

fn classify_command(command: &str) -> Option<Operation> {
    // Shell words, split on separators so `a && scripts/release-train.sh …`
    // still reads the release-train invocation.
    for segment in command.split(['\n', ';', '&', '|']) {
        let words: Vec<&str> = segment.split_whitespace().collect();
        let program = |index: usize| {
            words
                .get(index)
                .map(|word| word.rsplit('/').next().unwrap_or(word))
        };
        let has = |flag: &str| words.iter().any(|word| *word == flag);
        for (index, word) in words.iter().enumerate() {
            let name = word.rsplit('/').next().unwrap_or(word);
            if name == "release-train.sh" && has("--cut") {
                return Some(Operation::ReleaseCut);
            }
            if name == "release-train.sh" && has("--check-lane") {
                return Some(Operation::Merge);
            }
            if name == "cas" && program(index + 1) == Some("release") && program(index + 2) == Some("report") {
                return Some(Operation::Report);
            }
            if name == "gh" && program(index + 1) == Some("pr") && program(index + 2) == Some("merge") {
                return Some(Operation::Merge);
            }
            if name == "git"
                && program(index + 1) == Some("merge")
                && !has("--abort")
                && !has("--continue")
                && !has("--quit")
            {
                return Some(Operation::Merge);
            }
        }
    }
    None
}

/// The tagged lessons for `operation`, as a short deterministic checklist.
pub(crate) fn checklist(entries: &[Entry], operation: Operation) -> Option<String> {
    let tag = operation.tag();
    let mut lessons: Vec<&Entry> = entries
        .iter()
        .filter(|entry| !entry.archived)
        .filter(|entry| entry.tags.iter().any(|candidate| tags_operation(candidate, operation)))
        .filter(|entry| {
            entry
                .valid_until
                .is_none_or(|until| until > chrono::Utc::now())
        })
        .collect();
    if lessons.is_empty() {
        return None;
    }
    lessons.sort_by(|left, right| {
        right
            .importance
            .total_cmp(&left.importance)
            .then_with(|| right.created.cmp(&left.created))
            .then_with(|| left.id.cmp(&right.id))
    });
    let total = lessons.len();
    let mut out = format!(
        "[operation lessons: {}] Before you continue, check each lesson tagged {tag}:",
        operation.name()
    );
    for entry in lessons.iter().take(MAX_LESSONS) {
        out.push_str(&format!("\n- [ ] [{}] {}", entry.id, summary(entry)));
    }
    if total > MAX_LESSONS {
        out.push_str(&format!(
            "\n({} more: memory action=list tags={tag})",
            total - MAX_LESSONS
        ));
    }
    Some(out)
}

/// `op:<name>`, or the long `operation:<name>` spelling some lessons already use.
fn tags_operation(tag: &str, operation: Operation) -> bool {
    let tag = tag.to_ascii_lowercase();
    tag.strip_prefix("op:")
        .or_else(|| tag.strip_prefix("operation:"))
        .is_some_and(|name| name == operation.name())
}

fn summary(entry: &Entry) -> String {
    let text = entry
        .title
        .as_deref()
        .filter(|title| !title.trim().is_empty())
        .unwrap_or(&entry.content);
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && *line != "---")
        .unwrap_or("");
    if line.chars().count() <= MAX_LESSON_CHARS {
        return line.to_string();
    }
    let mut clipped: String = line.chars().take(MAX_LESSON_CHARS - 1).collect();
    clipped.push('…');
    clipped
}

fn state_path(cas_root: &Path, session_id: &str, operation: Operation) -> PathBuf {
    let session: String = session_id
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' { ch } else { '_' })
        .collect();
    cas_root
        .join(STATE_DIR)
        .join(format!("{}.{}", if session.is_empty() { "unknown" } else { &session }, operation.name()))
}

/// Whether this session surfaced `operation` recently; records it if not.
fn claim_surfacing(cas_root: &Path, session_id: &str, operation: Operation) -> bool {
    let path = state_path(cas_root, session_id, operation);
    let recent = std::fs::metadata(&path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age < RESURFACE_AFTER);
    if recent {
        return false;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, b"");
    true
}

/// PostToolUse context for a tool call that starts a tagged operation.
pub(crate) fn context_for_tool(cas_root: &Path, input: &HookInput) -> Option<String> {
    let prefix = crate::harness_policy::own_tool_prefix();
    let operation = classify(input, prefix, |id| {
        crate::store::open_task_store(cas_root)
            .ok()
            .and_then(|store| store.get(id).ok())
            .is_some_and(|task| task.task_type == crate::types::TaskType::Epic)
    })?;
    let entries = crate::store::open_store(cas_root).ok()?.list().ok()?;
    let context = checklist(&entries, operation)?;
    claim_surfacing(cas_root, &input.session_id, operation).then_some(context)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{SqliteStore, Store};
    use tempfile::TempDir;

    fn input(tool: &str, tool_input: serde_json::Value) -> HookInput {
        serde_json::from_value(serde_json::json!({
            "session_id": "session-operation-lessons",
            "transcript_path": "/tmp/operation-lessons.jsonl",
            "cwd": "/tmp",
            "hook_event_name": "PostToolUse",
            "tool_name": tool,
            "tool_input": tool_input,
        }))
        .unwrap()
    }

    fn bash(command: &str) -> Option<Operation> {
        classify(&input("Bash", serde_json::json!({ "command": command })), "mcp__cas__", |_| false)
    }

    #[test]
    fn classifies_operation_starts_literally() {
        assert_eq!(
            bash("scripts/release-train.sh 9.99.3 /wt --cut > run.log 2>&1 &"),
            Some(Operation::ReleaseCut)
        );
        assert_eq!(
            bash("cd /wt && ./scripts/release-train.sh 9.99.3 /wt --cut"),
            Some(Operation::ReleaseCut)
        );
        assert_eq!(bash("scripts/release-train.sh 9.99.3 /wt --check-lane factory/x"), Some(Operation::Merge));
        assert_eq!(bash("git merge --no-ff factory/gentle-kestrel-51"), Some(Operation::Merge));
        assert_eq!(bash("gh pr merge 1193 --squash"), Some(Operation::Merge));
        assert_eq!(bash("cas release report 9.99.0 --pdf"), Some(Operation::Report));
        assert_eq!(bash("git merge --abort"), None);
        assert_eq!(bash("git merge-base HEAD main"), None);
        assert_eq!(bash("grep -n release-train.sh docs/x.md"), None);
        assert_eq!(bash("scripts/release-train.sh 9.99.3 /wt --status"), None);

        let skill = |name: &str| {
            classify(&input("Skill", serde_json::json!({ "skill": name })), "mcp__cas__", |_| false)
        };
        assert_eq!(skill("cas-cut-release"), Some(Operation::ReleaseCut));
        assert_eq!(skill("cas:cas-html-reports"), Some(Operation::Report));
        assert_eq!(skill("cas-worker"), None);

        let spawn = input(
            "mcp__cs__factory",
            serde_json::json!({ "action": "spawn_workers", "count": 1 }),
        );
        assert_eq!(classify(&spawn, "mcp__cs__", |_| false), Some(Operation::Spawn));
        assert_eq!(classify(&spawn, "mcp__cas__", |_| false), None);

        let close = input("mcp__cas__task", serde_json::json!({ "action": "close", "id": "cas-a229" }));
        assert_eq!(classify(&close, "mcp__cas__", |id| id == "cas-a229"), Some(Operation::EpicClose));
        assert_eq!(classify(&close, "mcp__cas__", |_| false), None);
    }

    fn lesson(store: &SqliteStore, title: &str, tags: &[&str], importance: f32) -> String {
        let entry = Entry {
            id: store.generate_id().unwrap(),
            title: Some(title.to_string()),
            content: format!("{title}. Body detail."),
            tags: tags.iter().map(|tag| tag.to_string()).collect(),
            importance,
            ..Default::default()
        };
        store.add(&entry).unwrap();
        entry.id
    }

    /// AC2: starting a release cut surfaces the release-cut lessons, by tag,
    /// as a checklist; an untagged lesson with the same words does not ride
    /// along, and the list is surfaced once per session window.
    #[test]
    fn release_cut_start_surfaces_tagged_release_lessons() {
        let temp = TempDir::new().unwrap();
        let store = SqliteStore::open(temp.path()).unwrap();
        store.init().unwrap();
        let scratch = lesson(
            &store,
            "Scratch base must live on the release checkout's filesystem",
            &["release", "op:release-cut"],
            0.9,
        );
        let literals = lesson(
            &store,
            "Tests use env!(\"CARGO_PKG_VERSION\"), never a literal version",
            &["op:release-cut"],
            0.6,
        );
        let untagged = lesson(&store, "Release cut notes without an operation tag", &["release"], 1.0);
        let report = lesson(&store, "Open every report in Chrome before sharing", &["op:report"], 0.8);

        let start = input(
            "Bash",
            serde_json::json!({ "command": "scripts/release-train.sh 9.99.3 /wt/release --cut > cut.log 2>&1 &" }),
        );
        let context = context_for_tool(temp.path(), &start).expect("release-cut lessons surfaced");
        assert!(context.starts_with("[operation lessons: release-cut]"), "{context}");
        let scratch_at = context.find(&scratch).expect("scratch-base lesson");
        let literals_at = context.find(&literals).expect("version-literals lesson");
        assert!(scratch_at < literals_at, "importance orders the checklist: {context}");
        assert!(!context.contains(&untagged), "{context}");
        assert!(!context.contains(&report), "{context}");

        // Same session, same operation, inside the window: not repeated.
        assert_eq!(context_for_tool(temp.path(), &start), None);
        // The skill load in a new session surfaces the same checklist.
        let mut skill = input("Skill", serde_json::json!({ "skill": "cas-cut-release" }));
        skill.session_id = "another-session".to_string();
        assert!(context_for_tool(temp.path(), &skill).unwrap().contains(&scratch));
        // A tool call that starts no operation surfaces nothing.
        assert_eq!(
            context_for_tool(temp.path(), &input("Bash", serde_json::json!({ "command": "ls" }))),
            None
        );
    }

    /// The PostToolUse hook carries the checklist to the agent.
    #[test]
    fn post_tool_use_hook_injects_the_release_cut_checklist() {
        let temp = TempDir::new().unwrap();
        let store = SqliteStore::open(temp.path()).unwrap();
        store.init().unwrap();
        let id = lesson(&store, "Pin the cut date from started_at", &["op:release-cut"], 0.7);
        let hook = input("Skill", serde_json::json!({ "skill": "cas-cut-release" }));
        let output = crate::hooks::handle_post_tool_use(&hook, Some(temp.path())).unwrap();
        let Some(crate::hooks::HookSpecificOutput::PostToolUse {
            additional_context: Some(context),
        }) = output.hook_specific_output
        else {
            panic!("release-cut skill load must surface its lessons");
        };
        assert!(context.contains("[operation lessons: release-cut]"), "{context}");
        assert!(context.contains(&id), "{context}");
    }

    #[test]
    fn checklist_is_bounded_and_points_at_the_rest() {
        let entries: Vec<Entry> = (0..9)
            .map(|index| Entry {
                id: format!("m-{index}"),
                content: format!("{} lesson {index}", "x".repeat(400)),
                tags: vec!["OP:Merge".to_string()],
                ..Default::default()
            })
            .collect();
        let list = checklist(&entries, Operation::Merge).unwrap();
        assert_eq!(list.matches("\n- [ ] ").count(), MAX_LESSONS);
        assert!(list.contains("(3 more: memory action=list tags=op:merge)"));
        assert!(list.lines().all(|line| line.chars().count() < 220), "{list}");
        assert_eq!(checklist(&entries, Operation::Spawn), None);
        let long = Entry {
            id: "long-tag".to_string(),
            content: "Never run assembly-proof concurrently with journey-eval".to_string(),
            tags: vec!["release".to_string(), "operation:release-cut".to_string()],
            ..Default::default()
        };
        assert!(checklist(&[long], Operation::ReleaseCut).unwrap().contains("[long-tag]"));
    }
}
