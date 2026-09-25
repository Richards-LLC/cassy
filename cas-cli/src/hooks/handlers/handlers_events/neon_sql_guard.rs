//! Worker Neon SQL write guard (cas-d8fc, GH #907).
//!
//! A factory worker seeded QA fixtures with `mcp__neon__run_sql` and no
//! `branchId`, and Neon ran the statements on the project's default branch:
//! production. Nothing guarded the Neon SQL tools. This guard denies a factory
//! worker's `run_sql` / `run_sql_transaction` when it carries a write
//! statement and resolves to production, meaning either:
//!
//! - no `branchId`: Neon uses the project's default branch, which is
//!   production; or
//! - a `branchId` that the repository's generated Neon skill file
//!   (`.claude/skills/neon-database/SKILL.md`, `neon-ids` keep block)
//!   records as the production branch, or the literal names `main` or
//!   `production`.
//!
//! Reads are unaffected, and so is a write to any other branch. The denial
//! names the branch the call resolved to. SQL classification fails closed: a
//! statement that is not recognisably read-only counts as a write.

use std::path::{Path, PathBuf};

/// The Neon SQL tools this guard covers, matched on the MCP tool-name suffix so
/// the server prefix (`mcp__neon__`, a connector prefix, ...) does not matter.
fn is_neon_sql_tool(tool_name: &str) -> bool {
    let lower = tool_name.to_ascii_lowercase();
    lower.contains("neon")
        && (lower.ends_with("__run_sql") || lower.ends_with("__run_sql_transaction"))
}

/// Every SQL string the call would execute (`sql`, or `sqlStatements` for a
/// transaction; snake_case spellings too).
fn sql_statements(tool_input: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    for key in ["sql", "query"] {
        if let Some(sql) = tool_input.get(key).and_then(|v| v.as_str()) {
            out.push(sql.to_string());
        }
    }
    for key in ["sqlStatements", "sql_statements", "statements"] {
        if let Some(list) = tool_input.get(key).and_then(|v| v.as_array()) {
            out.extend(list.iter().filter_map(|v| v.as_str()).map(str::to_string));
        }
    }
    out
}

fn string_param<'a>(tool_input: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| tool_input.get(*key).and_then(|v| v.as_str()))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// Blank out comments, string literals, quoted identifiers and dollar-quoted
/// bodies, so keywords inside them never count.
fn strip_sql_noise(sql: &str) -> String {
    let chars: Vec<char> = sql.chars().collect();
    let mut out = String::with_capacity(sql.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '-' && next == Some('-') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            out.push(' ');
            continue;
        }
        if c == '/' && next == Some('*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
            out.push(' ');
            continue;
        }
        if c == '\'' || c == '"' {
            i += 1;
            while i < chars.len() {
                if chars[i] == c {
                    if chars.get(i + 1) == Some(&c) {
                        i += 2;
                        continue;
                    }
                    break;
                }
                i += 1;
            }
            i += 1;
            out.push(' ');
            continue;
        }
        if c == '$' {
            // $tag$ ... $tag$ (tag may be empty).
            let mut j = i + 1;
            while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_') {
                j += 1;
            }
            if j < chars.len() && chars[j] == '$' {
                let tag: String = chars[i..=j].iter().collect();
                let rest: String = chars[j + 1..].iter().collect();
                match rest.find(&tag) {
                    Some(end) => {
                        i = j + 1 + rest[..end].chars().count() + tag.chars().count();
                    }
                    None => i = chars.len(),
                }
                out.push(' ');
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Statement keywords that never change data.
const NEUTRAL: &[&str] = &[
    "BEGIN",
    "START",
    "COMMIT",
    "END",
    "ROLLBACK",
    "SAVEPOINT",
    "RELEASE",
    "SHOW",
    "SET",
    "RESET",
];
/// Statements that read, unless they carry a data-modifying clause.
const READ: &[&str] = &["SELECT", "WITH", "VALUES", "TABLE", "EXPLAIN"];
/// Data-modifying words that turn a read-shaped statement into a write.
const MODIFYING: &[&str] = &[
    "INSERT", "UPDATE", "DELETE", "MERGE", "TRUNCATE", "DROP", "ALTER", "CREATE", "SETVAL",
    "NEXTVAL",
];

/// The first keyword of the first statement that writes, or `None` when every
/// statement is read-only. Unrecognised statements count as writes.
pub(crate) fn first_write_keyword(sql: &str) -> Option<String> {
    let cleaned = strip_sql_noise(sql);
    for statement in cleaned.split(';') {
        let words: Vec<String> = statement
            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .filter(|w| !w.is_empty())
            .map(|w| w.to_ascii_uppercase())
            .collect();
        let Some(first) = words.first() else { continue };
        if NEUTRAL.contains(&first.as_str()) {
            continue;
        }
        if READ.contains(&first.as_str()) {
            // Row locks ("FOR UPDATE", "FOR NO KEY UPDATE") read; they do not modify.
            let modifying = words.iter().enumerate().any(|(index, word)| {
                if word == "UPDATE"
                    && index > 0
                    && matches!(words[index - 1].as_str(), "FOR" | "KEY")
                {
                    return false;
                }
                MODIFYING.contains(&word.as_str())
            });
            // SELECT ... INTO <table> creates a table.
            let select_into = first != "EXPLAIN" && words.iter().any(|w| w == "INTO");
            if modifying || select_into {
                return Some(first.clone());
            }
            continue;
        }
        return Some(first.clone());
    }
    None
}

/// Production branch ids recorded by `cas integrate neon` in the repo's skill
/// files, with the file each came from, for the given project (or any project
/// when the call names none).
fn recorded_production_branches(
    roots: &[PathBuf],
    project_id: Option<&str>,
) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for root in roots {
        for relative in [
            ".claude/skills/neon-database/SKILL.md",
            ".cursor/skills/neon-database/SKILL.md",
        ] {
            let path = root.join(relative);
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Some((recorded_project, ids)) =
                crate::cli::integrate::neon::recorded_production_branches(&text)
            else {
                continue;
            };
            if project_id.is_some_and(|project| project != recorded_project) {
                continue;
            }
            out.extend(ids.into_iter().map(|id| (id, path.clone())));
        }
    }
    out
}

/// The deny reason for a worker's Neon SQL call that writes to production, or
/// `None` when the call is allowed (a read, or a write to another branch).
pub(crate) fn neon_production_write_denial(
    tool_name: &str,
    tool_input: Option<&serde_json::Value>,
    roots: &[PathBuf],
) -> Option<String> {
    if !is_neon_sql_tool(tool_name) {
        return None;
    }
    let tool_input = tool_input?;
    let keyword = sql_statements(tool_input)
        .iter()
        .find_map(|sql| first_write_keyword(sql))?;
    let project_id = string_param(tool_input, &["projectId", "project_id"]);
    let branch = string_param(tool_input, &["branchId", "branch_id", "branch"]);
    let resolved = match branch {
        None => "no branchId, so Neon runs it on the project's default branch, which is production"
            .to_string(),
        Some(branch) if matches!(branch.to_ascii_lowercase().as_str(), "main" | "production") => {
            format!("branch `{branch}`, the production branch name")
        }
        Some(branch) => {
            let recorded = recorded_production_branches(roots, project_id);
            let (_, source) = recorded.iter().find(|(id, _)| id == branch)?;
            format!(
                "branch `{branch}`, which {} records as the production branch",
                display_path(source, roots)
            )
        }
    };
    Some(format!(
        "🚫 NEON PRODUCTION WRITE: this {tool_name} call carries a write statement ({keyword}) and resolved to {resolved}. \
         Workers may write only to a non-production branch.\n\n\
         Workers cannot create Neon branches. Ask the supervisor for one: \
         `{coord} action=message target=supervisor blocker=true summary=\"db branch for <task-id>\" message=\"...\"`; \
         its db_branch_create writes DATABASE_URL to .env.cas-db in your worktree. \
         If a non-production branch is already recorded for this task, pass its id as `branchId`. \
         Read-only statements (SELECT, SHOW, EXPLAIN) need no branch and are not blocked.",
        coord = format!("{}coordination", crate::harness_policy::own_tool_prefix()),
    ))
}

fn display_path(path: &Path, roots: &[PathBuf]) -> String {
    roots
        .iter()
        .find_map(|root| path.strip_prefix(root).ok())
        .unwrap_or(path)
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_reads_and_writes() {
        for read in [
            "SELECT * FROM users",
            "select id, updated_at from t where comment is null",
            "WITH x AS (SELECT 1) SELECT * FROM x",
            "EXPLAIN ANALYZE SELECT 1",
            "SHOW search_path",
            "BEGIN; SELECT 1; COMMIT;",
            "SELECT * FROM jobs FOR UPDATE SKIP LOCKED",
            "SELECT 'DELETE FROM users' AS note -- DROP TABLE x",
            "SELECT $$INSERT INTO t$$ /* UPDATE */",
            "  \n-- a comment\nSELECT 1;",
        ] {
            assert_eq!(first_write_keyword(read), None, "{read}");
        }
        for (write, keyword) in [
            ("INSERT INTO users(id) VALUES (1)", "INSERT"),
            ("update users set name = 'x'", "UPDATE"),
            ("DELETE FROM users", "DELETE"),
            ("CREATE TABLE t(id int)", "CREATE"),
            ("DROP TABLE t", "DROP"),
            ("TRUNCATE t", "TRUNCATE"),
            (
                "WITH gone AS (DELETE FROM t RETURNING *) SELECT * FROM gone",
                "WITH",
            ),
            ("SELECT * INTO backup FROM users", "SELECT"),
            ("SELECT 1; INSERT INTO t VALUES (2)", "INSERT"),
            ("SELECT setval('users_id_seq', 10)", "SELECT"),
            ("GRANT ALL ON t TO bob", "GRANT"),
            ("COPY t FROM '/tmp/x'", "COPY"),
            ("DO $$ BEGIN PERFORM 1; END $$", "DO"),
            ("vacuum", "VACUUM"),
        ] {
            assert_eq!(
                first_write_keyword(write).as_deref(),
                Some(keyword),
                "{write}"
            );
        }
    }

    fn skill(dir: &Path, project: &str, production: &str) {
        let path = dir.join(".claude/skills/neon-database");
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(
            path.join("SKILL.md"),
            format!(
                "# Neon\n\n<!-- keep neon-ids -->\n| | Value |\n|--|--|\n| **org_id** | `org-1` |\n| **projectId** | `{project}` |\n| **databaseName** | `neondb` |\n| **production branchId** | `{production}` (name: `main`) |\n| **staging branchId** | `br-staging` (name: `staging`) |\n<!-- /keep neon-ids -->\n"
            ),
        )
        .unwrap();
    }

    #[test]
    fn denies_production_writes_and_names_the_branch() {
        let temp = tempfile::tempdir().unwrap();
        skill(temp.path(), "proj-1", "br-prod");
        let roots = vec![temp.path().to_path_buf()];
        let tool = "mcp__neon__run_sql";
        let no_branch =
            serde_json::json!({"projectId": "proj-1", "sql": "INSERT INTO t VALUES (1)"});
        let reason = neon_production_write_denial(tool, Some(&no_branch), &roots)
            .expect("no branchId writes are denied");
        assert!(
            reason.contains("no branchId") && reason.contains("production"),
            "{reason}"
        );
        assert!(reason.contains("(INSERT)"), "{reason}");
        // cas-90e8: the remedy is one a worker can take — a blocker message
        // to the supervisor, who owns db_branch_create — never a branch create.
        assert!(
            reason.contains("coordination action=message target=supervisor blocker=true summary="),
            "{reason}"
        );
        assert!(!reason.contains("create_branch"), "{reason}");

        let prod = serde_json::json!({"projectId": "proj-1", "branchId": "br-prod", "sql": "DELETE FROM t"});
        let reason = neon_production_write_denial(tool, Some(&prod), &roots)
            .expect("the recorded production branch is denied");
        assert!(
            reason.contains("`br-prod`")
                && reason.contains(".claude/skills/neon-database/SKILL.md"),
            "{reason}"
        );

        let main = serde_json::json!({"branchId": "main", "sql": "UPDATE t SET x = 1"});
        assert!(
            neon_production_write_denial(tool, Some(&main), &roots)
                .unwrap()
                .contains("`main`")
        );

        let transaction = serde_json::json!({"projectId": "proj-1", "sqlStatements": ["SELECT 1", "INSERT INTO t VALUES (2)"]});
        assert!(
            neon_production_write_denial(
                "mcp__neon__run_sql_transaction",
                Some(&transaction),
                &roots
            )
            .is_some()
        );
    }

    #[test]
    fn allows_reads_and_writes_to_other_branches() {
        let temp = tempfile::tempdir().unwrap();
        skill(temp.path(), "proj-1", "br-prod");
        let roots = vec![temp.path().to_path_buf()];
        let tool = "mcp__neon__run_sql";
        for input in [
            serde_json::json!({"projectId": "proj-1", "sql": "SELECT count(*) FROM users"}),
            serde_json::json!({"projectId": "proj-1", "branchId": "br-staging", "sql": "INSERT INTO t VALUES (1)"}),
            serde_json::json!({"projectId": "proj-1", "branchId": "br-scratch-42", "sql": "DROP TABLE t"}),
            // A production id recorded for another project does not match this one.
            serde_json::json!({"projectId": "proj-2", "branchId": "br-prod", "sql": "INSERT INTO t VALUES (1)"}),
        ] {
            assert_eq!(
                neon_production_write_denial(tool, Some(&input), &roots),
                None,
                "{input}"
            );
        }
        // Other tools are not this guard's business.
        let describe = serde_json::json!({"sql": "DROP TABLE t"});
        assert_eq!(
            neon_production_write_denial("mcp__neon__describe_branch", Some(&describe), &roots),
            None
        );
        assert_eq!(
            neon_production_write_denial("Bash", Some(&describe), &roots),
            None
        );
    }
}
