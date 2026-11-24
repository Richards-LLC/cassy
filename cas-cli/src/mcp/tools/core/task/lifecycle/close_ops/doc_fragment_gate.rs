//! Doc fragments at park (cas-3cf7). Release prep folds per-task fragments
//! into the shared docs (cas-4b33), so a product delivery without its
//! overview fragment, or a bug fix on a live journey without its bug family,
//! used to park and then lose a verifier round. Park now names both misses.
use super::*;
use std::path::Path;

const OVERVIEW_DIR: &str = "docs/product-overview.d";

/// Test files ship nothing a reader of the overview or a journey would see.
fn test_path(path: &str) -> bool {
    let file = path.rsplit('/').next().unwrap_or(path);
    path.split('/').any(|part| matches!(part, "tests" | "__tests__" | "fixtures"))
        || file == "tests.rs"
        || file.ends_with("_tests.rs")
        || file.ends_with("_test.rs")
        || file.contains(".test.")
        || file.contains(".spec.")
}

pub(super) fn park_refusals(
    cas_root: &Path,
    repo: &Path,
    task: &Task,
    changed_paths: &[String],
    head: &str,
) -> Vec<String> {
    let product: Vec<&str> = changed_paths
        .iter()
        .map(String::as_str)
        .filter(|path| receipt_scope::product_source_path(path) && !test_path(path))
        .collect();
    if product.is_empty() {
        return Vec::new();
    }
    let Ok(tracked) = registry_upkeep::tracked(repo, head) else {
        return Vec::new();
    };
    let caller = crate::mcp::tools::core::guidance::caller_prefix();
    let short = &head[..head.len().min(12)];
    let id = &task.id;
    let mut refusals = Vec::new();

    // gate[G-167 park as-of]: product delivery needs its docs/product-overview.d fragment
    let overview = format!("{OVERVIEW_DIR}/{id}.md");
    if tracked.iter().any(|path| path.starts_with(&format!("{OVERVIEW_DIR}/")))
        && !tracked.contains(&overview)
        && !receipt_scope::has_reasoned_decision(&task.notes, "no overview:")
    {
        refusals.push(crate::gates::tag("G-167", format!(
            "⚠️ OVERVIEW FRAGMENT REQUIRED: task {id} changes product source ({}) but {short} has no `{overview}`. Release prep folds that fragment into docs/PRODUCT_OVERVIEW.md.\n\nNext: add `{overview}` with `## <Section>` headings that already exist in docs/PRODUCT_OVERVIEW.md, each followed by the text to append, commit, push and retry `{caller}task action=close id={id}`. For a change with nothing to describe, record `{caller}task action=notes id={id} note_type=decision notes=\"no overview: <reason>\"`.",
            product.join(", "),
        )));
    }

    if task.task_type == TaskType::Bug
        && !receipt_scope::has_reasoned_decision(&task.notes, "no bug family:")
    {
        let qa = crate::config::Config::load(cas_root).map(|config| config.qa()).unwrap_or_default();
        let changes: Vec<_> = product.iter().map(|path| crate::qa_impact::Change::modified(*path)).collect();
        let live: Vec<String> = crate::qa_impact::analyze(repo, &qa, &changes, None, Some(head))
            .map(|impact| impact.journeys.into_iter()
                .filter(|journey| journey.status == "live" && !journey.matches.is_empty())
                .map(|journey| journey.id)
                .collect())
            .unwrap_or_default();
        let named = || crate::qa_registry::load_with(&qa.registry.path, &tracked, |path| {
            registry_upkeep::read(repo, head, path, &tracked)
        })
        .ok()
        .flatten()
        .is_some_and(|registry| registry.bug_families.values()
            .any(|family| family.tasks.contains(id) || family.fixed_by.contains(id)));
        // gate[G-168 park as-of]: bug fix on a live journey's owned files needs a bug family naming it
        if !live.is_empty() && !named() {
            refusals.push(crate::gates::tag("G-168", format!(
                "⚠️ BUG FAMILY REQUIRED: bug {id} changes files owned by live journey(s) {} but no bug family at {short} names {id}.\n\nNext: add `docs/qa/bug-families/<slug>.yaml` with `title`, `tasks: [{id}]` and the affected `journeys`, commit, push and retry `{caller}task action=close id={id}`. When the fix cannot regress a journey, record `{caller}task action=notes id={id} note_type=decision notes=\"no bug family: <reason>\"`.",
                live.join(", "),
            )));
        }
    }
    refusals
}

#[cfg(test)]
#[path = "doc_fragment_gate_tests.rs"]
mod tests;
