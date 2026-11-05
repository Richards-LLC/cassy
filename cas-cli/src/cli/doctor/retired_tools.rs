//! References to MCP servers that are no longer registered (cas-1ad4d).

use super::{Check, CheckStatus};
use std::path::Path;

pub(super) fn check(_cas_root: &Path) -> Check {
    Check::new("retired MCP references", CheckStatus::Ok, "no unavailable MCP references")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{init_cas_dir, open_rule_store_local, open_skill_store_local, open_store_local};
    use crate::types::{Entry, Rule, Skill};
    use std::fs;

    #[test]
    fn doctor_lists_removed_mcp_references_by_id_and_file_cas_1ad4d() {
        let env = crate::test_support::TestEnvGuard::temp_home();
        let project = env.home().join("project");
        fs::create_dir_all(&project).unwrap();
        let root = init_cas_dir(&project).unwrap();
        open_rule_store_local(&root).unwrap().add(&Rule::new(
            "rule-removed".into(), "Use the Neon MCP run_sql tool".into(),
        )).unwrap();
        open_store_local(&root).unwrap().add(&Entry::new(
            "mem-removed".into(), "Call mcp__neon__run_sql".into(),
        )).unwrap();
        let mut skill = Skill::new("skill-removed".into(), "old database tool".into());
        skill.invocation = "mcp__neon__run_sql".into();
        open_skill_store_local(&root).unwrap().add(&skill).unwrap();
        let brief = root.join("templates/worker-brief.md");
        fs::create_dir_all(brief.parent().unwrap()).unwrap();
        fs::write(&brief, "Use the Vercel MCP for deployment").unwrap();
        let row = check(&root);
        assert!(matches!(row.status, CheckStatus::Warning), "{}", row.message);
        for source in ["rule-removed", "mem-removed", "skill-removed", "templates/worker-brief.md"] {
            assert!(row.message.contains(source), "missing {source}: {}", row.message);
        }
        assert!(row.message.contains("neon") && row.message.contains("vercel"), "{}", row.message);
        // Registration restores availability without editing the guidance.
        fs::write(root.join("proxy.toml"), "[servers.neon]\ntransport = 'http'\nurl = 'https://example.test/neon'\n[servers.vercel]\ntransport = 'http'\nurl = 'https://example.test/vercel'\n").unwrap();
        let available = check(&root);
        assert!(!available.message.contains("rule-removed") && !available.message.contains("mem-removed")
            && !available.message.contains("skill-removed") && !available.message.contains("templates/worker-brief.md"), "{}", available.message);
    }
}
