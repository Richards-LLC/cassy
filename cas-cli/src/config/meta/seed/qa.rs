use crate::config::meta::registry::ConfigRegistry;
use crate::config::meta::types::{ConfigMeta, ConfigType, Constraint};

pub(super) fn register_qa(registry: &mut ConfigRegistry) {
    for (key, name) in [("upkeep.codemap", "CODEMAP Upkeep Stage"), ("upkeep.product_overview", "Product Overview Upkeep Stage")] {
        registry.register(ConfigMeta {
            key, section: "upkeep", name,
            description: "Delivery-scoped docs gate at park. off disables; observe records would-refusals; enforce refuses new drift. Setting observe explicitly pins it beyond the one-release adoption window.",
            value_type: ConfigType::String, default: "enforce",
            constraint: Constraint::OneOf(vec!["off".into(), "observe".into(), "enforce".into()]),
            advanced: false, requires_feature: None,
            keywords: &["docs", "codemap", "overview", "upkeep", "observe"],
            use_cases: &["Keep navigation and product documentation current with the delivered diff"],
        });
    }
    registry.register(ConfigMeta {
        key: "qa.registry.enforce", section: "qa", name: "Registry Upkeep Stage",
        description: "G1–G3 at park: off, observe (durable would-refusals), enforce (new surfaces), strict (also modified unowned surfaces). Strict is opt-in only.",
        value_type: ConfigType::String, default: "enforce",
        constraint: Constraint::OneOf(vec!["off".into(), "observe".into(), "enforce".into(), "strict".into()]),
        advanced: false, requires_feature: None,
        keywords: &["qa", "journey", "registry", "upkeep", "observe", "strict"],
        use_cases: &["Roll out registry upkeep gates and measure would-refusals"],
    });
    registry.register(ConfigMeta {
        key: "qa.registry.path",
        section: "qa",
        name: "Journey Registry Path",
        description: "Project-relative YAML journey registry validated by cas qa journeys --check.",
        value_type: ConfigType::String,
        default: "docs/qa/journeys.yaml",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["qa", "journey", "registry", "yaml"],
        use_cases: &["Choose a project-specific journey registry location"],
    });

    registry.register(ConfigMeta {
        key: "qa.post_release.command", section: "qa", name: "Post-release command",
        description: "Project-relative read-only executable and whitespace-separated arguments; no shell.",
        value_type: ConfigType::String, default: "", constraint: Constraint::None,
        advanced: true, requires_feature: None,
        keywords: &["qa", "post-release", "canary"],
        use_cases: &["Observe a release before announcing it or closing its epic"],
    });
    registry.register(ConfigMeta {
        key: "qa.post_release.window_mins", section: "qa", name: "Post-release window mins",
        description: "Observation window in minutes.",
        value_type: ConfigType::Int, default: "30", constraint: Constraint::Min(1),
        advanced: true, requires_feature: None,
        keywords: &["qa", "post-release", "canary"],
        use_cases: &["Observe a release before announcing it or closing its epic"],
    });
    registry.register(ConfigMeta {
        key: "qa.post_release.interval_mins", section: "qa", name: "Post-release interval mins",
        description: "Minutes between observations, including first and final runs.",
        value_type: ConfigType::Int, default: "15", constraint: Constraint::Min(1),
        advanced: true, requires_feature: None,
        keywords: &["qa", "post-release", "canary"],
        use_cases: &["Observe a release before announcing it or closing its epic"],
    });
    registry.register(ConfigMeta {
        key: "qa.post_release.timeout_secs", section: "qa", name: "Post-release timeout secs",
        description: "Per-run timeout; a timeout fails the check.",
        value_type: ConfigType::Int, default: "120", constraint: Constraint::Min(1),
        advanced: true, requires_feature: None,
        keywords: &["qa", "post-release", "canary"],
        use_cases: &["Observe a release before announcing it or closing its epic"],
    });
    registry.register(ConfigMeta {
        key: "qa.post_release.release_branches", section: "qa", name: "Post-release release branches",
        description: "Release branches; empty uses the origin default branch.",
        value_type: ConfigType::StringList, default: "", constraint: Constraint::None,
        advanced: true, requires_feature: None,
        keywords: &["qa", "post-release", "canary"],
        use_cases: &["Observe a release before announcing it or closing its epic"],
    });
    registry.register(ConfigMeta {
        key: "qa.post_release.env", section: "qa", name: "Post-release env",
        description: "Environment variable names allowed into the read-only command.",
        value_type: ConfigType::StringList, default: "", constraint: Constraint::None,
        advanced: true, requires_feature: None,
        keywords: &["qa", "post-release", "canary"],
        use_cases: &["Observe a release before announcing it or closing its epic"],
    });
    for (key, name, description, value_type, default, constraint) in [
        ("qa.release.integration_branch", "Release integration branch", "Branch the release gate walks, for example staging.", ConfigType::String, "", Constraint::None),
        ("qa.release.branches", "Release promotion branches", "Promotion targets whose PRs need a clean release gate.", ConfigType::StringList, "", Constraint::None),
        ("qa.release.deploy_workflows", "Release deploy workflows", "GitHub Actions workflow names watched for deployment failures on the release integration branch.", ConfigType::StringList, "", Constraint::None),
        ("qa.release.adapter", "Release adapter", "Project-relative QA adapter executable, called as `<adapter> <verb>` with protocol v1 JSON on stdin and stdout; no shell. Empty leaves the gate off.", ConfigType::String, "", Constraint::None),
        ("qa.release.env", "Release adapter env", "Environment variable names allowed into the adapter.", ConfigType::StringList, "", Constraint::None),
        ("qa.release.timeout_secs", "Release adapter timeout secs", "Per-call adapter timeout.", ConfigType::Int, "120", Constraint::Min(1)),
        ("qa.release.max_cells", "Release golden-set row limit", "Most rows (cells × runs) one golden-set journey may expand into; a larger expansion refuses the gate at start.", ConfigType::Int, "500", Constraint::Min(1)),
        ("qa.release.retention_days", "Release media retention days", "Days evidence-pack media stay on local disk; pack.json is kept.", ConfigType::Int, "30", Constraint::Min(1)),
        ("qa.release.components", "Release served components", "JSON object of served component name to repository paths, for served-SHA equivalence.", ConfigType::String, "{}", Constraint::None),
        ("qa.release.target_env", "Release target env", "Environment promotion serves (for example prod); the release gate reads its `flags` once and compares them with the pack's snapshot.", ConfigType::String, "prod", Constraint::None),
        ("qa.release.gate_env", "Release gate env", "Environment the release gate walks, sent as every adapter request's env.", ConfigType::String, "staging", Constraint::None),
        ("qa.release.suites", "Release suites", "JSON object of suite name to {journeys or kinds, wall_clock_mins, concurrency}; rows past the wall clock are NOT_EXERCISED{timed_out}.", ConfigType::String, "{}", Constraint::None),
        ("qa.release.grant_envs", "Release grant envs", "Environments where release-gate preflight may call the adapter's `grant` verb to cover a balance shortfall; elsewhere a shortfall refuses.", ConfigType::StringList, "staging", Constraint::None),
        ("qa.release.migrations.paths", "Release migration paths", "Repository-relative patterns with exactly one `*` (the migration id). A merge into integration_branch that adds a match needs the adapter's `migrations` verb to report it applied.", ConfigType::StringList, "", Constraint::None),
        ("qa.release.migrations.env", "Release migration env", "Environment whose applied migrations a merge into integration_branch needs, for example staging.", ConfigType::String, "", Constraint::None),
    ] {
        registry.register(ConfigMeta {
            key, section: "qa", name, description, value_type, default, constraint,
            advanced: true, requires_feature: None,
            keywords: &["qa", "release-gate", "adapter", "staging"],
            use_cases: &["Gate staging-to-production promotion on one evidence pack"],
        });
    }
    registry.register(ConfigMeta {
        key: "qa.mode", section: "qa", name: "QA Mode",
        description: "per_task keeps per-delivery evidence and independent QA. batch defers evidence at park and accepts one published staging ledger for its listed delivery anchors after integration; no automatic per-task QA is dispatched. Explicit QA requests remain available.",
        value_type: ConfigType::String, default: "per_task",
        constraint: Constraint::OneOf(vec!["per_task".into(), "batch".into()]),
        advanced: false, requires_feature: None,
        keywords: &["qa", "batch", "staging", "ledger", "receipt"],
        use_cases: &["Review a batch once on deployed staging before closing its tasks"],
    });
    registry.register(ConfigMeta {
        key: "qa.user_facing_labels",
        section: "qa",
        name: "User-Facing Task Labels",
        description: "Comma-separated labels that require a non-empty demo_statement when creating a task. Epics and supervisor overrides are exempt.",
        value_type: ConfigType::StringList,
        default: "ui,hub,hub-web,cli-ux,commander,frontend",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["qa", "user-facing", "labels", "demo", "statement", "task"],
        use_cases: &[
            "Add project-specific labels such as mobile or public-api",
            "Clear the list when no labels should opt into the creation gate",
        ],
    });

    registry.register(ConfigMeta {
        key: "qa.telemetry_sweep",
        section: "qa",
        name: "Telemetry Sweep Command",
        description: "Optional project-relative read-only command run by cas-qa-craft before its exploration matrix. It must emit one tab-delimited finding per line and never print secrets or raw event payloads.",
        value_type: ConfigType::String,
        default: "",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["qa", "telemetry", "sweep", "findings", "posthog", "blackout"],
        use_cases: &[
            "Run a project-owned analytics sweep before user-flow QA",
            "Leave empty to record sweep: not configured in the QA ledger",
        ],
    });

    registry.register(ConfigMeta {
        key: "qa.independent_pass",
        section: "qa",
        name: "Independent QA Pass",
        description: "Before a user-facing factory delivery merges, dispatch an independent QA and polish pass run by a taste-lane worker who is not the implementer. Merge and close wait for its verdict.",
        value_type: ConfigType::Bool,
        default: "true",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["qa", "independent", "polish", "review", "merge", "taste"],
        use_cases: &[
            "Disable for a project with no user-facing surface",
            "Keep enabled so easy-to-spot bugs are caught before merge",
        ],
    });

    registry.register(ConfigMeta {
        key: "qa.deployed_origins",
        section: "qa",
        name: "QA Deployed Origins",
        description: "Comma-separated remote origins (https://staging.example.com) whose authenticated runs may stand in for a local build in a QA evidence bundle when local auth is impossible, for example when the staging backend's CORS rejects localhost. The bundle must record the reason and prove the deployment served the delivered commit.",
        value_type: ConfigType::StringList,
        default: "",
        constraint: Constraint::None,
        advanced: true,
        requires_feature: None,
        keywords: &["qa", "evidence", "deployed", "staging", "origin", "cors", "auth", "bundle"],
        use_cases: &["Accept staging evidence for pages that need a staging login"],
    });

    registry.register(ConfigMeta {
        key: "qa.preflight_gh_token",
        section: "qa",
        name: "QA Preflight: GitHub Read Token",
        description: "Before an independent QA reviewer claims a round, require a GitHub read token in its environment (GH_TOKEN or GITHUB_TOKEN, or an authenticated gh). A missing token refuses the start with a blocker; the value is never shown.",
        value_type: ConfigType::Bool,
        default: "false",
        constraint: Constraint::None,
        advanced: true,
        requires_feature: None,
        keywords: &["qa", "preflight", "github", "token", "gh", "reviewer", "credentials"],
        use_cases: &["Enable when reviewers read PRs or issues through gh"],
    });

    registry.register(ConfigMeta {
        key: "qa.preflight_env_files",
        section: "qa",
        name: "QA Preflight: Env Files",
        description: "Comma-separated environment variable names that must point at a readable file in the reviewer's environment, such as the backend env file used to mint staging QA sessions. Only the path is checked and reported; the file is never read.",
        value_type: ConfigType::StringList,
        default: "",
        constraint: Constraint::None,
        advanced: true,
        requires_feature: None,
        keywords: &["qa", "preflight", "env", "file", "staging", "session", "reviewer"],
        use_cases: &["Require GABBER_BACKEND_ENV_FILE before a staging QA round"],
    });

    registry.register(ConfigMeta {
        key: "qa.preflight_hook",
        section: "qa",
        name: "QA Preflight: Capacity Hook",
        description: "Project-relative command run before a reviewer claims a round, with CAS_QA_DELIVERY_TASK, CAS_QA_TASK and CAS_QA_HEAD set. It checks and may replenish test-account capacity (for example staging credits). Exit 0 is ready; any other exit refuses the start, showing the first output line. Output is redacted and must not contain secrets.",
        value_type: ConfigType::String,
        default: "",
        constraint: Constraint::None,
        advanced: true,
        requires_feature: None,
        keywords: &["qa", "preflight", "hook", "credits", "capacity", "staging", "account", "top-up"],
        use_cases: &["Top up the staging QA account's credits before each round"],
    });

    registry.register(ConfigMeta {
        key: "qa.preflight_hook_timeout_secs",
        section: "qa",
        name: "QA Preflight: Hook Timeout",
        description: "Seconds the QA preflight hook may run before it counts as a blocker.",
        value_type: ConfigType::Int,
        default: "120",
        constraint: Constraint::Min(1),
        advanced: true,
        requires_feature: None,
        keywords: &["qa", "preflight", "hook", "timeout"],
        use_cases: &["Raise for a slow billing API"],
    });

    registry.register(ConfigMeta {
        key: "qa.github_status",
        section: "qa",
        name: "GitHub QA Status",
        description: "Publish the cassy/independent-qa commit status on each delivered head: pending while an independent QA round is open, success once it passes or is waived, failure on rejection. Require that context in branch protection so a merge on GitHub waits for the verdict too.",
        value_type: ConfigType::Bool,
        default: "false",
        constraint: Constraint::None,
        advanced: true,
        requires_feature: None,
        keywords: &["qa", "github", "status", "required", "check", "branch", "protection", "merge"],
        use_cases: &[
            "Enable for a project that merges factory deliveries as GitHub pull requests",
            "Leave disabled when every merge goes through worktree_merge",
        ],
    });

    registry.register(ConfigMeta {
        key: "qa.evidence_gate",
        section: "qa",
        name: "QA Evidence Close Gate",
        description: "Refuse the implementer's close of a user-facing delivery until its cas-qa-craft evidence bundle is valid for the delivered commit (trace with a passing assertion, screencast receipt, final aria snapshot, polish renders, visual QA PASS, critique floor). Demo-only deliveries with no web surface need the evidence ledger instead. Also refuses deliveries that add test.fixme/skip/only markers without a cas-allow-skip reason.",
        value_type: ConfigType::Bool,
        default: "true",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["qa", "evidence", "bundle", "close", "gate", "playwright", "trace", "fixme", "skip"],
        use_cases: &[
            "Keep enabled so user-facing deliveries cannot close without proof they were run",
            "Disable for a project with no user-facing surface",
        ],
    });

    registry.register(ConfigMeta {
        key: "qa.terminal_render_paths",
        section: "qa",
        name: "Terminal Rendering Paths",
        description: "Comma-separated repo-relative globs for terminal output. A demo-only delivery touching one needs terminal-qa PASS plus its real-build ledger, except for interactive surfaces in qa.terminal_interaction_paths. CLI command paths and mux pane rendering still need terminal-qa.",
        value_type: ConfigType::String,
        default: "**/cli/**,**/ui/**,**/tui/**,**/terminal/**,**/doctor.rs,**/doctor/**,**/cas-mux/**,**/ghostty_vt/**,**/ghostty_vt_sys/**,**/cas-diffs/**",
        constraint: Constraint::None,
        advanced: true,
        requires_feature: None,
        keywords: &["qa", "terminal", "cli", "render", "evidence", "terminal-qa"],
        use_cases: &[
            "Name the modules that draw your CLI or TUI output",
            "Leave empty to never require a terminal-qa receipt",
        ],
    });

    registry.register(ConfigMeta {
        key: "qa.terminal_interaction_paths",
        section: "qa",
        name: "Interactive Terminal Paths",
        description: "Comma-separated repo-relative globs for interactive TUI and PTY surfaces. These require a fresh real-build PASS ledger instead of a command stdout terminal-qa receipt. Other output paths in a mixed delivery, CLI command paths and mux pane rendering still need terminal-qa. Leave empty to require receipts for every terminal_render_paths match.",
        value_type: ConfigType::String,
        default: "**/ui/factory/**,**/cas-pty/**,**/cas-mux/**",
        constraint: Constraint::None,
        advanced: true,
        requires_feature: None,
        keywords: &["qa", "terminal", "tui", "pty", "input", "geometry", "ledger"],
        use_cases: &[
            "Exercise input and PTY geometry through the real interactive application",
            "Name interactive modules separately from command output modules",
        ],
    });

    registry.register(ConfigMeta {
        key: "qa.user_facing_paths",
        section: "qa",
        name: "User-Facing Paths",
        description: "Comma-separated repo-relative globs. A factory delivery whose diff touches one needs the independent QA pass even without a label or demo_statement.",
        value_type: ConfigType::StringList,
        default: "**/*.html,**/*.css,**/*.scss,**/*.vue,**/*.svelte,**/*.tsx,**/*.jsx",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["qa", "user-facing", "paths", "globs", "surface", "independent"],
        use_cases: &[
            "Add a web app directory such as hub-web/**",
            "Clear the list so only labels and demo statements select the pass",
        ],
    });

    registry.register(ConfigMeta {
        key: "qa.api_paths",
        section: "qa",
        name: "API QA Paths",
        description: "Comma-separated globs for API and server-only code with no rendered output. Matches trigger HTTP transcript and vendored contract diff QA instead of visual evidence, including server-only TSX. Other rendered paths in a mixed delivery still require a web bundle.",
        value_type: ConfigType::StringList,
        default: "",
        constraint: Constraint::None,
        advanced: false,
        requires_feature: None,
        keywords: &["qa", "api", "http", "contract", "server", "paths"],
        use_cases: &["Select app/api/**/route.ts and lib/server/**", "Keep rendered components out of API-only globs"],
    });

    registry.register(ConfigMeta {
        key: "qa.pass_timeout_mins",
        section: "qa",
        name: "QA Pass Timeout",
        description: "Minutes one independent QA round may take before it times out and the supervisor must redispatch or waive it.",
        value_type: ConfigType::Int,
        default: "45",
        constraint: Constraint::Min(1),
        advanced: true,
        requires_feature: None,
        keywords: &["qa", "timeout", "independent", "round", "cost"],
        use_cases: &["Raise for slow builds", "Lower to keep QA rounds short"],
    });

    registry.register(ConfigMeta {
        key: "qa.max_rounds",
        section: "qa",
        name: "QA Pass Round Limit",
        description: "Rejected independent QA rounds before Cassy escalates to the supervisor instead of opening another round.",
        value_type: ConfigType::Int,
        default: "3",
        constraint: Constraint::Min(1),
        advanced: true,
        requires_feature: None,
        keywords: &["qa", "rounds", "independent", "escalate", "cost"],
        use_cases: &["Escalate sooner on churny deliveries"],
    });
}
