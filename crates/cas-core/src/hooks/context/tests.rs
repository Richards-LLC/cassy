use crate::hooks::context::{
    BasicContextScorer, ContextQuery, ContextScorer, RuleMatchCache, estimate_tokens,
    is_factory_participant, remap_tool_prefix, rule_matches_path, select_task_titles,
    token_display, truncate,
};
use cas_types::{AgentRole, Entry, EntryType, Rule, RuleStatus, Task};

#[test]
fn test_estimate_tokens() {
    assert_eq!(estimate_tokens("test"), 1);
    assert_eq!(estimate_tokens("12345678"), 2);
    assert_eq!(estimate_tokens(""), 0);
}

#[test]
fn test_token_display() {
    assert_eq!(token_display(50), "~50tk");
    assert_eq!(token_display(150), "~150tk");
    assert_eq!(token_display(1500), "~1.5k tk");
}

#[test]
fn test_truncate() {
    assert_eq!(truncate("short", 10), "short");
    assert_eq!(truncate("this is a long string", 10), "this is...");
}

#[test]
fn test_truncate_handles_unicode_boundary() {
    let input = format!("{}✅ done", "a".repeat(99));
    assert_eq!(truncate(&input, 103), format!("{}...", "a".repeat(99)));
}

#[test]
fn external_mcp_tool_names_are_preserved_on_every_prompt_surface() {
    let prompt = "external: mcp__viktor__ask_viktor mcp__viktor-shadow__ask_viktor mcp__foreign__read_file; CAS: mcp__cas__task";
    for (surface, cas_prefix) in [
        ("Claude", "mcp__cas__"),
        ("Codex", "mcp__cs__"),
        ("Grok", "cas__"),
    ] {
        let rendered = remap_tool_prefix(prompt, cas_prefix);
        assert!(
            rendered.contains("mcp__viktor__ask_viktor"),
            "{surface} prompt must preserve the explicit Viktor tool"
        );
        assert!(
            rendered.contains("mcp__viktor-shadow__ask_viktor"),
            "{surface} prompt must preserve a lookalike external server"
        );
        assert!(
            rendered.contains("mcp__foreign__read_file"),
            "{surface} prompt must preserve a foreign external tool"
        );
        assert!(
            rendered.contains(&format!("{cas_prefix}task")),
            "{surface} prompt must still remap only the CAS tool"
        );
    }
}

#[test]
fn test_rule_matches_path() {
    let mut rule = Rule {
        paths: String::new(),
        ..Default::default()
    };
    assert!(rule_matches_path(&rule, "/any/path"));

    rule.paths = "src/**".to_string();
    assert!(rule_matches_path(&rule, "/project/src/main.rs"));

    rule.paths = "lib/cas_cloud/**".to_string();
    assert!(rule_matches_path(&rule, "/project/lib/cas_cloud/web"));
}

#[test]
fn test_basic_context_scorer() {
    let entry = Entry {
        entry_type: EntryType::Learning,
        created: chrono::Utc::now(),
        ..Default::default()
    };

    let score = BasicContextScorer::calculate_score(&entry);
    assert!(score > 0.0);

    // Learning should score higher than Observation
    let obs = Entry {
        entry_type: EntryType::Observation,
        created: chrono::Utc::now(),
        ..Default::default()
    };

    assert!(
        BasicContextScorer::calculate_score(&entry) > BasicContextScorer::calculate_score(&obs)
    );
}

#[test]
fn test_context_query_to_string() {
    let query = ContextQuery {
        task_titles: vec!["Fix bug in parser".to_string()],
        cwd: "/home/user/my-project".to_string(),
        user_prompt: Some("help me debug".to_string()),
        recent_files: vec![],
        git_branch: None,
    };

    let query_str = query.to_query_string();
    assert!(query_str.contains("Fix bug in parser"));
    assert!(query_str.contains("help me debug"));
    assert!(query_str.contains("my-project"));
}

#[test]
fn test_basic_scorer_trait() {
    let scorer = BasicContextScorer;
    assert_eq!(scorer.name(), "basic");

    let entries = vec![
        Entry {
            id: "1".to_string(),
            entry_type: EntryType::Learning,
            created: chrono::Utc::now(),
            ..Default::default()
        },
        Entry {
            id: "2".to_string(),
            entry_type: EntryType::Observation,
            created: chrono::Utc::now(),
            ..Default::default()
        },
    ];

    let context = ContextQuery::default();
    let scored = scorer.score_entries(&entries, &context);

    assert_eq!(scored.len(), 2);
    // Learning should be first (higher score)
    assert_eq!(scored[0].0.id, "1");
}

#[test]
fn test_context_query_has_content() {
    // Empty query has no content
    let empty = ContextQuery::default();
    assert!(!empty.has_content());

    // Query with task titles has content
    let with_task = ContextQuery {
        task_titles: vec!["Fix bug".to_string()],
        ..Default::default()
    };
    assert!(with_task.has_content());

    // Query with user prompt has content
    let with_prompt = ContextQuery {
        user_prompt: Some("help me debug".to_string()),
        ..Default::default()
    };
    assert!(with_prompt.has_content());

    // Project identity is content: a fresh session with nothing but a cwd is
    // still a session *about a project*, and cas-3b80 measured that treating it
    // as empty is what pinned the production Helpful-Memories ranking to a
    // single query-blind list (distinct top-5 = 1 across all 56 eval cases).
    let with_cwd = ContextQuery {
        cwd: "/project".to_string(),
        ..Default::default()
    };
    assert!(with_cwd.has_content());

    // A checked-out branch is content on its own.
    let with_branch = ContextQuery {
        git_branch: Some("epic/memory-effectiveness".to_string()),
        ..Default::default()
    };
    assert!(with_branch.has_content());

    // A branch that contributes no usable term is not content.
    let generic_branch = ContextQuery {
        git_branch: Some("main".to_string()),
        ..Default::default()
    };
    assert!(!generic_branch.has_content());
}

#[test]
fn context_query_includes_branch_terms_but_drops_generic_ones() {
    let query = ContextQuery {
        git_branch: Some("epic/memory-effectiveness-program".to_string()),
        ..Default::default()
    };
    let text = query.to_query_string();
    assert!(
        text.contains("memory"),
        "branch terms missing from {text:?}"
    );
    assert!(text.contains("effectiveness"));
    assert!(
        !text.contains("epic"),
        "generic branch prefix leaked into {text:?}"
    );
}

#[test]
fn task_titles_prefer_the_reading_agent_over_the_whole_project() {
    let mut mine = Task::new(
        "cas-1111".to_string(),
        "Fix the retrieval scorer".to_string(),
    );
    mine.assignee = Some("agent-me".to_string());
    let mut theirs = Task::new(
        "cas-2222".to_string(),
        "Rewrite the release script".to_string(),
    );
    theirs.assignee = Some("agent-other".to_string());
    let unassigned = Task::new("cas-3333".to_string(), "Audit the tier filter".to_string());

    let tasks = vec![mine.clone(), theirs.clone(), unassigned.clone()];

    // With an identity, only the reader's own in-progress work seeds the query:
    // four concurrent factory lanes must not soup each other's titles together.
    assert_eq!(
        select_task_titles(&tasks, Some("agent-me")),
        vec!["Fix the retrieval scorer".to_string()]
    );

    // No task of one's own → project-wide titles remain the fallback rather
    // than leaving the query empty.
    assert_eq!(select_task_titles(&tasks, Some("agent-nobody")).len(), 3);
    assert_eq!(select_task_titles(&tasks, None).len(), 3);
}

#[test]
fn test_rule_match_cache() {
    let rule1 = Rule {
        id: "rule-1".to_string(),
        paths: "src/**".to_string(),
        ..Default::default()
    };

    let rule2 = Rule {
        id: "rule-2".to_string(),
        paths: "lib/**".to_string(),
        ..Default::default()
    };

    let rule3 = Rule {
        id: "rule-3".to_string(),
        paths: String::new(), // Matches everywhere
        ..Default::default()
    };

    let rules = vec![rule1.clone(), rule2.clone(), rule3.clone()];
    let cache = RuleMatchCache::build(&rules, "/project/src/main.rs");

    // Rule1 should match (src/**)
    assert!(cache.matches(&rule1, "/project/src/main.rs"));
    // Rule2 should not match (lib/**)
    assert!(!cache.matches(&rule2, "/project/src/main.rs"));
    // Rule3 should match (no path restriction)
    assert!(cache.matches(&rule3, "/project/src/main.rs"));

    // Cache should be valid for same cwd
    assert!(cache.is_valid_for("/project/src/main.rs"));
    // Cache should not be valid for different cwd
    assert!(!cache.is_valid_for("/other/path"));

    // Cache should have 3 entries
    assert_eq!(cache.len(), 3);
}

#[test]
fn test_rule_match_cache_fallback() {
    let rule = Rule {
        id: "rule-1".to_string(),
        paths: "src/**".to_string(),
        ..Default::default()
    };

    // Empty cache
    let cache = RuleMatchCache::new();
    assert!(cache.is_empty());

    // Should fall back to direct matching when cwd doesn't match cache
    // This also tests the behavior when the cache was built for a different cwd
    assert!(cache.matches(&rule, "/project/src/main.rs"));
}

#[test]
fn test_session_aware_access_boost() {
    let now = chrono::Utc::now();

    // Entry with recent access (within 1 hour) should get high boost
    let recent_entry = Entry {
        id: "recent".to_string(),
        entry_type: EntryType::Learning,
        created: now - chrono::Duration::days(7),
        last_accessed: Some(now - chrono::Duration::minutes(30)),
        access_count: 1,
        ..Default::default()
    };

    // Entry accessed 12 hours ago should get medium boost
    let medium_entry = Entry {
        id: "medium".to_string(),
        entry_type: EntryType::Learning,
        created: now - chrono::Duration::days(7),
        last_accessed: Some(now - chrono::Duration::hours(12)),
        access_count: 1,
        ..Default::default()
    };

    // Entry never accessed should get no boost
    let no_access_entry = Entry {
        id: "no_access".to_string(),
        entry_type: EntryType::Learning,
        created: now - chrono::Duration::days(7),
        last_accessed: None,
        access_count: 0,
        ..Default::default()
    };

    let recent_score = BasicContextScorer::calculate_score(&recent_entry);
    let medium_score = BasicContextScorer::calculate_score(&medium_entry);
    let no_access_score = BasicContextScorer::calculate_score(&no_access_entry);

    // Recent access should give highest score
    assert!(
        recent_score > medium_score,
        "Recent access should score higher than medium: {recent_score} vs {medium_score}"
    );
    assert!(
        medium_score > no_access_score,
        "Medium access should score higher than no access: {medium_score} vs {no_access_score}"
    );
}

#[test]
fn test_is_factory_participant() {
    assert!(is_factory_participant(Some(AgentRole::Worker)));
    assert!(is_factory_participant(Some(AgentRole::Supervisor)));
    assert!(!is_factory_participant(Some(AgentRole::Standard)));
    assert!(!is_factory_participant(Some(AgentRole::Director)));
    assert!(!is_factory_participant(None));
}

// ============================================================================
// Distilled-knowledge index injection (EPIC cas-7d31 / cas-86b2)
//
// SessionStart injects the knowledge *index* and toolizes the body: page
// titles + snippets + ids go into the prompt, prose does not. These tests pin
// the three properties that make that shape safe — no bodies, bounded size,
// and byte-stability for prompt caching.
// ============================================================================

use crate::hooks::config::DefaultHooksConfig;
use crate::hooks::context::{ContextStores, build_context_with_stores};
use crate::hooks::types::HookInput;
use cas_store::{
    IngestBatch, KnowledgePage, KnowledgeStore, PageWrite, RuleStore, SqliteKnowledgeStore,
    SqliteRuleStore, SqliteStore, Store,
};

/// A knowledge store in a throwaway dir, holding `pages` of (type, title,
/// snippet, body).
fn knowledge_fixture(
    pages: &[(&str, &str, &str, &str)],
) -> (tempfile::TempDir, SqliteKnowledgeStore) {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = SqliteKnowledgeStore::open(temp.path()).expect("open knowledge store");
    let writes: Vec<PageWrite> = pages
        .iter()
        .map(|(page_type, title, snippet, body)| {
            let id = store.generate_id().expect("generate id");
            let mut page = KnowledgePage::new(id, *page_type, *title);
            page.snippet = (*snippet).to_string();
            page.sources = vec!["docs/source.md".to_string()];
            PageWrite {
                page,
                body: (*body).to_string(),
            }
        })
        .collect();
    store
        .commit_ingest(&IngestBatch {
            pages: writes,
            ..Default::default()
        })
        .expect("commit ingest");
    (temp, store)
}

fn session_start_input() -> HookInput {
    HookInput {
        cwd: "/project".to_string(),
        hook_event_name: "SessionStart".to_string(),
        ..Default::default()
    }
}

#[test]
fn session_start_increments_surface_count_for_each_injected_rule() {
    let mut rule = Rule::new(
        "rule-surface".to_string(),
        "Always test surfaced rules".to_string(),
    );
    rule.status = RuleStatus::Proven;
    let temp = tempfile::tempdir().expect("tempdir");
    let store = SqliteRuleStore::open(temp.path()).expect("open rule store");
    store.init().expect("init rule store");
    store.add(&rule).expect("add rule");
    let stores = ContextStores {
        project_rule_store: Some(&store),
        ..ContextStores::empty()
    };

    let config = DefaultHooksConfig::new();
    let (context, stats) = build_context_with_stores(
        &session_start_input(),
        &stores,
        &config,
        10,
        None,
        "mcp__cas__",
    )
    .expect("build context");

    assert!(context.contains("Always test surfaced rules"));
    assert_eq!(stats.rules_included, 1);
    assert_eq!(store.get("rule-surface").unwrap().surface_count, 1);
}

fn build_with_knowledge(ks: &dyn KnowledgeStore, store: Option<&dyn Store>) -> String {
    let stores = ContextStores {
        project_store: store,
        knowledge_store: Some(ks),
        ..ContextStores::empty()
    };
    let config = DefaultHooksConfig::new();
    let (context, _) = build_context_with_stores(
        &session_start_input(),
        &stores,
        &config,
        10,
        None,
        "mcp__cas__",
    )
    .expect("build context");
    context
}

#[test]
fn session_start_injects_the_knowledge_index_and_a_pull_instruction() {
    let (_temp, ks) = knowledge_fixture(&[(
        "subsystem",
        "Task Verifier",
        "Gates task close on evidence.",
        "THE VERIFIER BODY PROSE that must never be injected verbatim.",
    )]);

    let context = build_with_knowledge(&ks, None);

    assert!(
        context.contains("## 📚 Project Knowledge"),
        "expected the knowledge index section, got:\n{context}"
    );
    assert!(
        context.contains("Task Verifier") && context.contains("Gates task close on evidence."),
        "expected title + snippet in the index, got:\n{context}"
    );
    assert!(
        context.contains("[subsystem]"),
        "expected the page type in the index, got:\n{context}"
    );
    // The whole point: the index points at the body, it does not carry it.
    assert!(
        !context.contains("THE VERIFIER BODY PROSE"),
        "page body leaked into the injected block:\n{context}"
    );
    // `contains("knowledge")` alone is not a test: the word appears in the
    // section header too, so that assertion stayed green while the instruction
    // advertised `action: show`, which the router rejects outright. Pin the
    // whole constant, and pin the action name separately so the failure message
    // says which half broke. `cas-cli`'s knowledge_tools suite proves the named
    // action is actually dispatchable.
    assert!(
        context.contains(super::KNOWLEDGE_PULL_INSTRUCTION),
        "expected the verbatim pull instruction, got:\n{context}"
    );
    assert!(
        super::KNOWLEDGE_PULL_INSTRUCTION.contains("action: read"),
        "the pull instruction must name a real `knowledge` action; got: {}",
        super::KNOWLEDGE_PULL_INSTRUCTION
    );
}

#[test]
fn the_injected_block_is_byte_identical_across_builds_on_an_unchanged_store() {
    let (_temp, ks) = knowledge_fixture(&[
        ("subsystem", "Task Verifier", "Gates close.", "body a"),
        ("architecture", "Build System", "Cargo workspace.", "body b"),
        ("workflow", "Release Cutting", "Bump, tag, notes.", "body c"),
    ]);

    let first = build_with_knowledge(&ks, None);
    let second = build_with_knowledge(&ks, None);

    assert_eq!(
        first, second,
        "injected block drifted between builds on an unchanged store; \
         a prompt-cache prefix must be byte-stable"
    );
}

#[test]
fn the_knowledge_index_is_ordered_by_type_then_title_not_by_insertion() {
    // Inserted in an order that is neither type-sorted nor title-sorted, so a
    // pass-through of store order would fail this.
    let (_temp, ks) = knowledge_fixture(&[
        ("workflow", "Zebra", "z", "body"),
        ("architecture", "Yak", "y", "body"),
        ("architecture", "Ant", "a", "body"),
    ]);

    let context = build_with_knowledge(&ks, None);
    let ant = context.find("Ant").expect("Ant present");
    let yak = context.find("Yak").expect("Yak present");
    let zebra = context.find("Zebra").expect("Zebra present");

    assert!(
        ant < yak,
        "architecture/Ant should precede architecture/Yak"
    );
    assert!(yak < zebra, "architecture/* should precede workflow/*");
}

#[test]
fn the_knowledge_index_respects_the_hook_token_budget() {
    // Far more pages than the section budget can hold.
    let long_snippet = "a snippet long enough to consume real budget ".repeat(3);
    let owned: Vec<(String, String, String, String)> = (0..200)
        .map(|i| {
            (
                "subsystem".to_string(),
                format!("Page {i:03}"),
                long_snippet.clone(),
                "body".to_string(),
            )
        })
        .collect();
    let pages: Vec<(&str, &str, &str, &str)> = owned
        .iter()
        .map(|(t, ti, s, b)| (t.as_str(), ti.as_str(), s.as_str(), b.as_str()))
        .collect();
    let (_temp, ks) = knowledge_fixture(&pages);

    let context = build_with_knowledge(&ks, None);

    let rendered = context.matches("[subsystem]").count();
    assert!(rendered > 0, "expected at least some pages rendered");
    assert!(
        rendered < 200,
        "index rendered all {rendered} pages, ignoring the section budget"
    );
    // Truncation must be reported, not silent.
    assert!(
        context.contains("/200 pages indexed"),
        "truncated index did not disclose how many pages it dropped:\n{context}"
    );
    assert!(
        estimate_tokens(&context) <= DefaultHooksConfig::new().token_budget,
        "injected block blew the hook token budget"
    );
}

#[test]
fn no_raw_memory_bodies_are_injected_alongside_the_knowledge_index() {
    // Regression guard for the index-inject/pull contract: non-pinned memories
    // are surfaced as previews, never as bodies. Pinned entries are the single
    // deliberate exception (persona/critical context, injected verbatim).
    let body = "UNPINNED MEMORY BODY that is far longer than the sixty character preview budget and must not appear.";
    let entry = Entry {
        id: "cas-mem1".to_string(),
        content: body.to_string(),
        entry_type: EntryType::Learning,
        created: chrono::Utc::now(),
        ..Default::default()
    };
    let (_temp, ks) = knowledge_fixture(&[("subsystem", "Verifier", "Gates close.", "kbody")]);
    // Same cas_dir as the knowledge fixture: one cas.db, as in a real project.
    let store = SqliteStore::open(_temp.path()).expect("open entry store");
    store.init().expect("init entry store");
    store.add(&entry).expect("add entry");

    let context = build_with_knowledge(&ks, Some(&store));

    // Guard against a vacuous pass: the memory must actually be surfaced —
    // as a pointer — for "no body" to mean anything.
    assert!(
        context.contains("cas-mem1"),
        "memory was not surfaced at all, so the no-body assertion proves nothing:\n{context}"
    );
    assert!(
        !context.contains(body),
        "a non-pinned memory body was injected verbatim:\n{context}"
    );
}

#[test]
fn cas_4caa_expired_memories_are_excluded_from_session_start_surfacing() {
    let expired = Entry {
        id: "expired-memory".to_string(),
        content: "Do not surface after its deadline".to_string(),
        entry_type: EntryType::Learning,
        created: chrono::Utc::now(),
        valid_until: Some(chrono::Utc::now() - chrono::Duration::seconds(1)),
        ..Default::default()
    };
    let active = Entry {
        id: "active-memory".to_string(),
        content: "This current fact should surface".to_string(),
        entry_type: EntryType::Learning,
        created: chrono::Utc::now(),
        ..Default::default()
    };
    let (_temp, ks) = knowledge_fixture(&[]);
    let store = SqliteStore::open(_temp.path()).expect("open entry store");
    store.init().expect("init entry store");
    store.add(&expired).expect("add expired memory");
    store.add(&active).expect("add active memory");

    let context = build_with_knowledge(&ks, Some(&store));
    assert!(
        context.contains("active-memory"),
        "active memory missing: {context}"
    );
    assert!(
        !context.contains("expired-memory"),
        "expired memory leaked into SessionStart: {context}"
    );
}
