//! Stop-hook maintenance job bodies (skills audit D12, M36, M48; L2 P1-58).
//!
//! The Stop hook queues four bounded one-shot jobs on the light lane:
//! learning-reviewer, rule-reviewer, duplicate-detector and
//! session-summarizer. They used to ship as agent definitions in all three
//! harness catalogs. Only the Codex copies were ever run (by
//! `include_str!`), the Claude and Grok copies were never spawned but were
//! listed in every session's agent list, and a Claude light lane would have
//! been handed the Codex tool names.
//!
//! Now each job has one body, written with the canonical `mcp__cas__` tool
//! prefix, kept outside every agent catalog so it is never installed as a
//! subagent. [`render_job_prompt_body`] remaps the prefix to whichever
//! harness the light lane resolves to when the prompt is built.

/// Tool prefix the job bodies are written with.
pub const CANONICAL_TOOL_PREFIX: &str = "mcp__cas__";

/// One Stop-hook maintenance job.
#[derive(Debug, Clone, Copy)]
pub struct MaintenanceJob {
    /// Job name, also its queue marker and log file stem.
    pub name: &'static str,
    /// Canonical job body (tool names use [`CANONICAL_TOOL_PREFIX`]).
    pub body: &'static str,
}

/// Every Stop-hook maintenance job.
pub const MAINTENANCE_JOBS: &[MaintenanceJob] = &[
    MaintenanceJob {
        name: "learning-reviewer",
        body: include_str!("builtins/jobs/learning-reviewer.md"),
    },
    MaintenanceJob {
        name: "rule-reviewer",
        body: include_str!("builtins/jobs/rule-reviewer.md"),
    },
    MaintenanceJob {
        name: "duplicate-detector",
        body: include_str!("builtins/jobs/duplicate-detector.md"),
    },
    MaintenanceJob {
        name: "session-summarizer",
        body: include_str!("builtins/jobs/session-summarizer.md"),
    },
];

/// The canonical body of the job called `name`.
pub fn job_body(name: &str) -> Option<&'static str> {
    MAINTENANCE_JOBS
        .iter()
        .find(|job| job.name == name)
        .map(|job| job.body)
}

/// A job body with its tool names rewritten to `tool_prefix`, the prefix of
/// the harness that will run it.
pub fn render_job_prompt_body(body: &str, tool_prefix: &str) -> String {
    body.replace(CANONICAL_TOOL_PREFIX, tool_prefix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_job_has_a_body_written_with_the_canonical_prefix() {
        for job in MAINTENANCE_JOBS {
            assert!(!job.body.trim().is_empty(), "{} body is empty", job.name);
            assert!(
                job.body.contains(CANONICAL_TOOL_PREFIX),
                "{} body must name tools with {CANONICAL_TOOL_PREFIX}",
                job.name
            );
            for foreign in ["mcp__cs__", " cas__", "`cas__"] {
                assert!(
                    !job.body.contains(foreign),
                    "{} body must not carry another harness prefix {foreign:?}",
                    job.name
                );
            }
            assert!(
                !job.body.starts_with("---"),
                "{} is a job body, not an agent definition",
                job.name
            );
        }
        assert_eq!(job_body("rule-reviewer"), Some(MAINTENANCE_JOBS[1].body));
        assert_eq!(job_body("task-verifier"), None);
    }

    #[test]
    fn render_remaps_every_tool_name_to_the_running_harness() {
        let body = job_body("learning-reviewer").unwrap();
        for prefix in ["mcp__cs__", "cas__", "mcp__cas__"] {
            let rendered = render_job_prompt_body(body, prefix);
            assert!(rendered.contains(&format!("{prefix}memory action=mark_reviewed")));
            if prefix != CANONICAL_TOOL_PREFIX {
                assert!(!rendered.contains(CANONICAL_TOOL_PREFIX), "{prefix}");
            }
        }
    }
}
