//! Operation-scoped lesson recall (cas-9734).
//!
//! Ambient recall matches a turn's words, so the release lessons lost to
//! whatever shared vocabulary with the prompt. Here the operation selects the
//! lessons: a memory tagged `operation:<op>` is surfaced as a checklist in
//! PostToolUse when a tool call starts that operation.

use std::path::Path;

use crate::hooks::HookInput;
use crate::types::Entry;

const MAX_LESSONS: usize = 6;
const MAX_LESSON_CHARS: usize = 160;

/// The operation a tool call starts: `release-cut`, `spawn`, `report` or
/// `epic-close`. `is_epic` is consulted only for a task close.
pub(crate) fn operation(
    input: &HookInput,
    tool_prefix: &str,
    is_epic: impl FnOnce(&str) -> bool,
) -> Option<&'static str> {
    let field = |name: &str| input.tool_input.as_ref()?.get(name)?.as_str();
    let tool = input.tool_name.as_deref()?;
    if tool == "Skill" {
        return match field("skill")?.rsplit(':').next()? {
            "cas-cut-release" => Some("release-cut"),
            "cas-release-report" | "cas-html-reports" => Some("report"),
            _ => None,
        };
    }
    if tool == "Bash" {
        let words: Vec<&str> = field("command")?.split_whitespace().collect();
        let starts = |program: &str, args: &[&str]| {
            words.iter().enumerate().any(|(index, word)| {
                word.rsplit('/').next() == Some(program)
                    && args
                        .iter()
                        .enumerate()
                        .all(|(offset, arg)| words.get(index + 1 + offset) == Some(arg))
            })
        };
        if starts("release.sh", &[])
            && !words.iter().any(|word| matches!(*word, "--next-version" | "--help" | "-h"))
        {
            return Some("release-cut");
        }
        return starts("cas", &["release", "report"]).then_some("report");
    }
    let action = field("action");
    if tool == format!("{tool_prefix}factory")
        && matches!(action, Some("spawn_workers" | "spawn_worker"))
    {
        return Some("spawn");
    }
    (tool == format!("{tool_prefix}task")
        && action == Some("close")
        && field("id").is_some_and(is_epic))
    .then_some("epic-close")
}

/// The lessons tagged `operation:<op>`, most important first.
pub(crate) fn checklist(entries: &[Entry], operation: &str) -> Option<String> {
    let tag = format!("operation:{operation}");
    let mut lessons: Vec<&Entry> = entries
        .iter()
        .filter(|entry| entry.tags.iter().any(|candidate| candidate.eq_ignore_ascii_case(&tag)))
        .collect();
    if lessons.is_empty() {
        return None;
    }
    lessons.sort_by(|left, right| {
        right
            .importance
            .total_cmp(&left.importance)
            .then_with(|| right.created.cmp(&left.created))
    });
    let mut out = format!("[operation lessons: {operation}] Check each before you continue:");
    for entry in lessons.iter().take(MAX_LESSONS) {
        let text = entry
            .title
            .as_deref()
            .filter(|title| !title.trim().is_empty())
            .unwrap_or(&entry.content);
        let line = text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("");
        let clipped: String = line.chars().take(MAX_LESSON_CHARS).collect();
        let ellipsis = if clipped.len() < line.len() { "…" } else { "" };
        out.push_str(&format!("\n- [ ] [{}] {clipped}{ellipsis}", entry.id));
    }
    if lessons.len() > MAX_LESSONS {
        out.push_str(&format!(
            "\n({} more: memory action=list tags={tag})",
            lessons.len() - MAX_LESSONS
        ));
    }
    Some(out)
}

/// PostToolUse context for a tool call that starts a tagged operation.
pub(crate) fn context_for_tool(cas_root: &Path, input: &HookInput) -> Option<String> {
    let operation = operation(input, crate::harness_policy::own_tool_prefix(), |id| {
        crate::store::open_task_store(cas_root)
            .ok()
            .and_then(|store| store.get(id).ok())
            .is_some_and(|task| task.task_type == crate::types::TaskType::Epic)
    })?;
    checklist(&crate::store::open_store(cas_root).ok()?.list().ok()?, operation)
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

    fn surfaced(root: &Path, hook: &HookInput) -> String {
        let output = crate::hooks::handle_post_tool_use(hook, Some(root)).unwrap();
        match output.hook_specific_output {
            Some(crate::hooks::HookSpecificOutput::PostToolUse {
                additional_context: Some(context),
            }) => context,
            _ => String::new(),
        }
    }

    /// AC2: starting a release cut surfaces the lessons tagged for it through
    /// the real PostToolUse hook; same-topic untagged memories do not ride along.
    #[test]
    fn release_cut_start_surfaces_tagged_release_lessons() {
        let temp = TempDir::new().unwrap();
        let store = SqliteStore::open(temp.path()).unwrap();
        store.init().unwrap();
        let scratch = lesson(
            &store,
            "Scratch base must live on the release checkout's filesystem",
            &["release", "operation:release-cut"],
            0.9,
        );
        let literals = lesson(
            &store,
            "Tests never contain the current workspace version",
            &["operation:release-cut"],
            0.6,
        );
        let untagged = lesson(&store, "Release cut notes without an operation tag", &["release"], 1.0);
        let report = lesson(&store, "Open every report in Chrome", &["operation:report"], 0.8);

        let cut = input(
            "Bash",
            serde_json::json!({ "command": "cd /wt && scripts/release.sh > cut.log 2>&1 &" }),
        );
        let context = surfaced(temp.path(), &cut);
        let scratch_at = context.find(&scratch).expect("scratch-base lesson surfaced");
        assert!(scratch_at < context.find(&literals).expect("version lesson surfaced"), "{context}");
        assert!(!context.contains(&untagged) && !context.contains(&report), "{context}");

        let skill = input("Skill", serde_json::json!({ "skill": "cas-cut-release" }));
        assert!(surfaced(temp.path(), &skill).contains(&scratch));
        let status = input(
            "Bash",
            serde_json::json!({ "command": "scripts/release.sh --next-version" }),
        );
        assert!(!surfaced(temp.path(), &status).contains("[operation lessons"));
    }

    #[test]
    fn spawn_lessons_match_harness_and_surface_once_per_session() {
        let temp = TempDir::new().unwrap();
        let store = SqliteStore::open(temp.path()).unwrap();
        store.init().unwrap();
        let codex = lesson(&store, "Codex spawn policy", &["operation:spawn", "codex"], 0.9);
        let general = lesson(&store, "Keep workers scoped", &["operation:spawn"], 0.8);
        let spawn = |cli: &str| input(&format!("{}factory", crate::harness_policy::own_tool_prefix()),
            serde_json::json!({"action": "spawn_workers", "cli": cli, "count": 1}));
        let claude = surfaced(temp.path(), &spawn("claude"));
        println!("NAG_MEASURE lessons_claude\n{claude}\nNAG_MEASURE_END");
        assert!(!claude.contains(&codex), "Claude spawn received Codex lesson: {claude}");
        assert!(claude.contains(&general), "{claude}");
        let first = surfaced(temp.path(), &spawn("codex"));
        assert!(first.contains(&codex) && !first.contains(&general), "{first}");
        assert!(!surfaced(temp.path(), &spawn("codex")).contains("[operation lessons"));
        let mut next = spawn("codex");
        next.session_id = "another-session".into();
        assert!(surfaced(temp.path(), &next).contains(&codex));
    }

    #[test]
    fn spawn_and_epic_close_are_recognized_on_the_callers_tool_prefix() {
        let spawn = input(
            "mcp__cs__factory",
            serde_json::json!({ "action": "spawn_workers", "count": 1 }),
        );
        assert_eq!(operation(&spawn, "mcp__cs__", |_| false), Some("spawn"));
        let close = input(
            "mcp__cas__task",
            serde_json::json!({ "action": "close", "id": "cas-a229" }),
        );
        assert_eq!(operation(&close, "mcp__cas__", |id| id == "cas-a229"), Some("epic-close"));
        assert_eq!(operation(&close, "mcp__cas__", |_| false), None);
    }
}
