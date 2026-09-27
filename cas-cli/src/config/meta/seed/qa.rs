use crate::config::meta::registry::ConfigRegistry;
use crate::config::meta::types::{ConfigMeta, ConfigType, Constraint};

pub(super) fn register_qa(registry: &mut ConfigRegistry) {
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
        description: "Comma-separated repo-relative globs. A demo-only (non-web) delivery whose diff touches one changes terminal rendering and needs a cas-cli-craft terminal-qa PASS receipt under the task artifacts dir, in addition to its evidence ledger.",
        value_type: ConfigType::String,
        default: "**/ui/**,**/tui/**,**/*render*,**/*output*,**/*theme*,**/*progress*",
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
