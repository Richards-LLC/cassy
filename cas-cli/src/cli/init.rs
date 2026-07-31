//! Initialize cas command with streamlined animated wizard
//!
//! Init flow:
//! 1. Welcome screen
//! 2. Confirmation with file summary
//! 3. Animated execution

use std::io::{Write, stdout};
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use clap::Parser;
use crossterm::{
    execute,
    style::{Color, Print, SetForegroundColor},
};
use tracing::{error, info, warn};

use crate::builtins::sync_all_builtins_for_harness;
use crate::config::{Config, HookConfig, SyncConfig, TasksConfig};
use crate::store::detect::init_cas_dir;

use crate::cli::Cli;
use crate::cli::factory_tooling;
use crate::cli::hook::{configure_claude_hooks, configure_codex_mcp_server, configure_mcp_server};
use crate::cli::interactive;

/// Overall timeout for `cas init`. If init is still running past this, the
/// watchdog aborts the process with a clear error so a hang never consumes
/// a CPU core indefinitely (see cas-bf06). Opt out via `CAS_INIT_NO_TIMEOUT=1`.
const INIT_TIMEOUT: Duration = Duration::from_secs(300);

/// Record a successfully initialized repository in the host registry.
///
/// Keep this host-scoped side effect at the CLI boundary. `init_cas_dir` is
/// also a public fixture primitive used by integration tests; making that
/// low-level helper write `~/.cas/cas.db` polluted developer registries and
/// created cross-test lock contention because integration-test dependencies
/// are compiled without `cfg(test)`.
fn register_initialized_repo(cwd: &Path) {
    if let Err(error) = crate::store::known_repos::ensure_host_schema() {
        warn!(
            error = %error,
            "failed to install host known_repos schema (non-fatal)",
        );
        return;
    }
    crate::store::known_repos::register_repo(cwd);
}

/// Spawn a watchdog thread that aborts the process if init runs longer than
/// `INIT_TIMEOUT`. The watchdog is purely defensive: normal init completes
/// in well under a second, so reaching the timeout always indicates a bug.
///
/// **Invariant:** this must only ever run in a short-lived process that exits
/// after `init::execute` returns (i.e., the `cas init` subcommand binary).
/// The spawned thread is intentionally detached and will call
/// `std::process::exit(3)` when its sleep elapses — it has no cancel channel.
/// That is safe today because all current callers (`cas init` CLI,
/// `bridge::server::factory::handle_factory_start`) invoke init as a
/// subprocess via `Command::new(...)`, so the process dies naturally on
/// success and the detached thread dies with it. If `init::execute` is ever
/// called in-process from a long-lived daemon, refactor this to use a
/// cancellable channel-based wait first.
fn spawn_init_watchdog() {
    if std::env::var("CAS_INIT_NO_TIMEOUT").ok().as_deref() == Some("1") {
        return;
    }
    thread::spawn(|| {
        thread::sleep(INIT_TIMEOUT);
        error!(
            timeout_secs = INIT_TIMEOUT.as_secs(),
            "cas init watchdog: aborting — init exceeded hard timeout. \
             Check .cas/logs/ for the last completed phase."
        );
        eprintln!(
            "\n\ncas init: aborting after {}s timeout. \
             Check .cas/logs/ for the last completed phase.\n\
             Set CAS_INIT_NO_TIMEOUT=1 to disable this watchdog \
             (e.g., in slow CI environments).",
            INIT_TIMEOUT.as_secs()
        );
        // Exit code 3 matches CasError::NotInitialized mapping in main.rs,
        // signalling "init did not complete successfully".
        std::process::exit(3);
    });
}

#[derive(Parser, Default)]
pub struct InitArgs {
    /// Accept all defaults without prompts
    #[arg(long, short = 'y')]
    pub yes: bool,

    /// Force reinitialize even if already initialized
    #[arg(long, short = 'f')]
    pub force: bool,

    /// Skip the Vercel/Neon/GitHub auto-integration section entirely.
    /// Equivalent to the `cas integrate <platform> init` flow not running.
    #[arg(long)]
    pub no_integrations: bool,

    /// Pre-seed the Vercel projectId; skips the picker. Still prompts to
    /// confirm in interactive mode.
    #[arg(long, value_name = "PROJECT_ID")]
    pub vercel: Option<String>,

    /// Pre-seed the Neon projectId; skips the picker.
    #[arg(long, value_name = "PROJECT_ID")]
    pub neon: Option<String>,

    /// Override the auto-detected GitHub `OWNER/REPO` (from `git remote -v`).
    #[arg(long, value_name = "OWNER/REPO")]
    pub github: Option<String>,
}

// ============================================================================
// Colors (CRT aesthetic matching boot.rs)
// ============================================================================

mod colors {
    use crossterm::style::Color;

    pub const CYAN: Color = Color::Rgb {
        r: 0,
        g: 200,
        b: 255,
    };
    pub const CYAN_BRIGHT: Color = Color::Rgb {
        r: 150,
        g: 230,
        b: 255,
    };
    pub const GREEN: Color = Color::Rgb {
        r: 80,
        g: 250,
        b: 120,
    };
    pub const ORANGE: Color = Color::Rgb {
        r: 255,
        g: 200,
        b: 80,
    };
    pub const RED: Color = Color::Rgb {
        r: 255,
        g: 90,
        b: 90,
    };
    pub const WHITE: Color = Color::White;
    pub const GRAY: Color = Color::Rgb {
        r: 120,
        g: 120,
        b: 130,
    };
    pub const DARK_GRAY: Color = Color::Rgb {
        r: 70,
        g: 70,
        b: 75,
    };
}

// Spinner frames (braille pattern)
const SPINNER_FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

// ============================================================================
// Configuration (simplified - uses smart defaults)
// ============================================================================

/// Agent selection for init configuration
#[derive(Clone, Copy, Debug)]
struct AgentSelection {
    claude: bool,
    codex: bool,
    /// EPIC cas-8888 (cas-6f46, Phase 5): opt-in Grok Build support.
    /// Unlike claude/codex, never auto-selected by `detect_agent_defaults`'s
    /// fresh-project heuristic — Grok support is new enough that an
    /// explicit choice (interactive prompt or `--grok`-equivalent flag)
    /// is safer than silently enabling it just because `grok` is on PATH.
    grok: bool,
}

/// Simplified wizard configuration
struct WizardConfig {
    agents: AgentSelection,
}

impl Default for WizardConfig {
    fn default() -> Self {
        Self {
            agents: AgentSelection {
                claude: true,
                codex: false,
                grok: false,
            },
        }
    }
}

impl WizardConfig {
    fn with_detected_agents(cwd: &Path) -> Self {
        Self {
            agents: detect_agent_defaults(cwd),
        }
    }

    /// Convert to full config with smart defaults
    fn to_config(&self) -> Config {
        let mut sync = SyncConfig {
            enabled: true,
            target: ".claude/rules/cas".to_string(),
            min_helpful: 1,
        };

        if self.agents.codex && !self.agents.claude {
            sync.target = ".codex/rules/cas".to_string();
        }

        Config {
            sync,
            hooks: Some(HookConfig {
                capture_enabled: true,
                capture_tools: vec!["Write".to_string(), "Edit".to_string(), "Bash".to_string()],
                inject_context: true,
                context_limit: 5,
                generate_summaries: false,
                token_budget: 4000,
                ai_context: false,
                ai_model: "claude-haiku-4-5".to_string(),
                plan_mode: Default::default(),
                minimal_start: false,
                ..Default::default()
            }),
            tasks: Some(TasksConfig {
                commit_nudge_on_close: false,
                block_exit_on_open: true,
            }),
            dev: None,
            code: None,
            cloud: None,
            notifications: None,
            agent: None,
            coordination: None,
            lease: None,
            verification: None,
            worktrees: None,
            theme: None,
            orchestration: None,
            factory: None,
            staging: None,
            telemetry: None,
            logging: None,
            llm: None,
            integrations: None,
            code_review: None,
            memory: None,
            project: None,
        }
    }
}

// ============================================================================
// Entry point
// ============================================================================

pub fn execute(args: &InitArgs, cli: &Cli) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;

    spawn_init_watchdog();
    info!(
        cwd = %cwd.display(),
        pid = std::process::id(),
        yes = args.yes,
        force = args.force,
        json = cli.json,
        "cas init: starting"
    );
    let started = Instant::now();

    // JSON mode: non-interactive, use defaults
    let result = if cli.json {
        execute_json(&cwd, args)
    } else if args.yes {
        // Yes mode: non-interactive, use defaults with text output
        execute_defaults(&cwd, args)
    } else {
        // Interactive wizard
        run_wizard(&cwd, args)
    };

    match &result {
        Ok(()) => info!(
            elapsed_ms = started.elapsed().as_millis() as u64,
            "cas init: completed"
        ),
        Err(e) => warn!(
            elapsed_ms = started.elapsed().as_millis() as u64,
            error = %e,
            "cas init: aborted with error"
        ),
    }
    result
}

// ============================================================================
// JSON mode (non-interactive)
// ============================================================================

fn execute_json(cwd: &Path, args: &InitArgs) -> anyhow::Result<()> {
    let cas_dir_path = cwd.join(".cas");

    if cas_dir_path.exists() && !args.force {
        println!(
            r#"{{"status":"already_initialized","path":"{}"}}"#,
            cas_dir_path.display()
        );
        return Ok(());
    }

    let cas_dir = init_cas_dir(cwd)?;
    register_initialized_repo(cwd);
    let config = WizardConfig::with_detected_agents(cwd);
    let config_data = config.to_config();
    let mut hooks_configured = false;
    let mut claude_md_updated = false;
    let mut skill_generated = false;
    let mut builtins_count = 0;
    let mut codex_configured = false;
    let mut grok_configured = false;
    let gitignore_updated = ensure_gitignore(cwd).is_ok();
    let _ = gitignore_updated;

    if config.agents.claude {
        hooks_configured = configure_claude_hooks(cwd, false).unwrap_or(false);
        claude_md_updated = update_claude_md(cwd).unwrap_or(false);
        skill_generated = generate_cas_skill(cwd).unwrap_or(false);

        let claude_dir = cwd.join(".claude");
        let builtins_result =
            sync_all_builtins_for_harness(cas_mux::SupervisorCli::Claude, &claude_dir).ok();
        builtins_count = builtins_result
            .as_ref()
            .map(|r| r.agents_updated + r.skills_updated)
            .unwrap_or(0);
    }

    if config.agents.codex {
        codex_configured = configure_codex_mcp_server(cwd).unwrap_or(false);

        let codex_dir = cwd.join(".codex");
        let builtins_result =
            sync_all_builtins_for_harness(cas_mux::SupervisorCli::Codex, &codex_dir).ok();
        builtins_count += builtins_result
            .as_ref()
            .map(|r| r.agents_updated + r.skills_updated)
            .unwrap_or(0);
    }

    if config.agents.grok {
        // Grok reads `.mcp.json` directly (verified via `grok mcp doctor`) —
        // no separate config writer needed, just make sure it exists.
        // Reuses the same idempotent writer Claude uses.
        grok_configured = configure_mcp_server(cwd).unwrap_or(false);

        let grok_dir = cwd.join(".grok");
        let builtins_result =
            sync_all_builtins_for_harness(cas_mux::SupervisorCli::Grok, &grok_dir).ok();
        builtins_count += builtins_result
            .as_ref()
            .map(|r| r.agents_updated + r.skills_updated)
            .unwrap_or(0);
    }

    // Setup factory tooling
    let factory_tooling_result = factory_tooling::setup_factory_tooling(cwd).unwrap_or_default();

    config_data.save(&cas_dir)?;

    let steps = next_steps_needed(cwd);

    println!(
        r#"{{"status":"initialized","path":"{}","agents":{},"hooks_configured":{},"claude_md_updated":{},"skill_generated":{},"builtins_synced":{},"codex_configured":{},"codex_hooks_review_required":{},"grok_configured":{},"factory_tooling":"{}","next_steps":{}}}"#,
        cas_dir.display(),
        serde_json::json!({
            "claude": config.agents.claude,
            "codex": config.agents.codex,
            "grok": config.agents.grok,
        }),
        hooks_configured,
        claude_md_updated,
        skill_generated,
        builtins_count,
        codex_configured,
        config.agents.codex,
        grok_configured,
        factory_tooling_result,
        serde_json::json!(steps),
    );

    Ok(())
}

// ============================================================================
// Defaults mode (--yes flag)
// ============================================================================

fn execute_defaults(cwd: &Path, args: &InitArgs) -> anyhow::Result<()> {
    let cas_dir_path = cwd.join(".cas");

    if cas_dir_path.exists() && !args.force {
        print_colored("", colors::WHITE)?;
        print_colored("  ● ", colors::CYAN)?;
        print_colored("CAS already initialized at ", colors::WHITE)?;
        print_colored(&cas_dir_path.display().to_string(), colors::CYAN)?;
        println!();
        print_colored("  → ", colors::GRAY)?;
        print_colored("Use ", colors::GRAY)?;
        print_colored("--force", colors::WHITE)?;
        print_colored(" to reinitialize\n\n", colors::GRAY)?;
        return Ok(());
    }

    // Mini header
    println!();
    print_colored("  █▀▀ ▄▀█ █▀ ", colors::CYAN_BRIGHT)?;
    print_colored("Init ", colors::WHITE)?;
    print_colored("(using defaults)\n", colors::GRAY)?;
    print_colored("  █▄▄ █▀█ ▄█\n\n", colors::CYAN_BRIGHT)?;

    let cas_dir = init_cas_dir(cwd)?;
    register_initialized_repo(cwd);

    // Apply with animation
    let config = WizardConfig::with_detected_agents(cwd);
    apply_configuration(&cas_dir, cwd, &config, false, &integration_flags_from(args))?;

    print_quick_start();
    print_next_steps(cwd);
    Ok(())
}

/// Translate CLI args into the orchestration layer's [`IntegrationFlags`].
fn integration_flags_from(args: &InitArgs) -> super::integrate::integrations::IntegrationFlags {
    super::integrate::integrations::IntegrationFlags {
        disabled: args.no_integrations,
        vercel_project: args.vercel.clone(),
        neon_project: args.neon.clone(),
        github_repo: args.github.clone(),
    }
}

// ============================================================================
// Interactive wizard (streamlined)
// ============================================================================

fn run_wizard(cwd: &Path, args: &InitArgs) -> anyhow::Result<()> {
    let cas_dir_path = cwd.join(".cas");

    // Welcome
    print_welcome()?;

    // Check if already initialized
    if cas_dir_path.exists() && !args.force {
        println!();
        print_colored("  CAS is already initialized at ", colors::WHITE)?;
        print_colored(&cas_dir_path.display().to_string(), colors::CYAN)?;
        println!("\n");

        let options = ["Reconfigure settings", "Keep existing and exit"];
        let choice = interactive::select("What would you like to do", &options)?;

        if choice == 1 {
            println!("\n  Keeping existing configuration.");
            return Ok(());
        }
        println!("\n  Reconfiguring CAS...");
    }

    // Initialize .cas directory
    let cas_dir = init_cas_dir(cwd)?;
    register_initialized_repo(cwd);
    let wizard_config = WizardConfig::with_detected_agents(cwd);

    // Confirmation with file summary
    if !confirm_and_apply(&cas_dir, cwd, &wizard_config, &integration_flags_from(args))? {
        println!("\n  Initialization cancelled.");
        return Ok(());
    }

    print_quick_start();
    print_next_steps(cwd);
    Ok(())
}

// ============================================================================
// Wizard sections
// ============================================================================

fn print_welcome() -> anyhow::Result<()> {
    println!();
    print_colored(
        "  ╭──────────────────────────────────────────────────────╮\n",
        colors::CYAN,
    )?;
    print_colored("  │", colors::CYAN)?;
    print_colored(
        "                                                      ",
        colors::CYAN,
    )?;
    print_colored("│\n", colors::CYAN)?;
    print_colored("  │  ", colors::CYAN)?;
    print_colored("█▀▀ ▄▀█ █▀", colors::CYAN_BRIGHT)?;
    print_colored("  Init", colors::WHITE)?;
    print_colored("                                     ", colors::CYAN)?;
    print_colored("│\n", colors::CYAN)?;
    print_colored("  │  ", colors::CYAN)?;
    print_colored("█▄▄ █▀█ ▄█", colors::CYAN_BRIGHT)?;
    print_colored("                                          ", colors::CYAN)?;
    print_colored("│\n", colors::CYAN)?;
    print_colored("  │", colors::CYAN)?;
    print_colored(
        "                                                      ",
        colors::CYAN,
    )?;
    print_colored("│\n", colors::CYAN)?;
    print_colored(
        "  ╰──────────────────────────────────────────────────────╯\n",
        colors::CYAN,
    )?;
    println!();
    print_colored("  │ ", colors::GRAY)?;
    print_colored(
        "Multi-agent coding factory with persistent memory and task coordination.\n",
        colors::WHITE,
    )?;
    println!();
    Ok(())
}

fn print_section_header(title: &str) -> anyhow::Result<()> {
    println!();
    print_colored("  ● ", colors::CYAN)?;
    print_colored(title, colors::WHITE)?;
    println!();
    print_colored("  ", colors::GRAY)?;
    print_colored(&"─".repeat(50), colors::DARK_GRAY)?;
    println!();
    Ok(())
}

fn is_claude_cli_installed() -> bool {
    std::process::Command::new("claude")
        .arg("--version")
        .output()
        .is_ok()
}

fn is_codex_cli_installed() -> bool {
    std::process::Command::new("codex")
        .arg("--version")
        .output()
        .is_ok()
}

fn detect_agent_defaults(cwd: &Path) -> AgentSelection {
    let claude = cwd.join(".claude").exists();
    let codex = cwd.join(".codex").exists();
    // Grok is opt-in only (see the AgentSelection.grok doc comment) — an
    // existing `.grok/` dir means a prior `cas init`/`cas update` already
    // opted this project in, so honor that; a fresh project never defaults
    // to grok=true regardless of whether the `grok` CLI is installed.
    let grok = cwd.join(".grok").exists();

    if !claude && !codex {
        // Fresh project: pick defaults from installed CLIs first.
        let claude_cli = is_claude_cli_installed();
        let codex_cli = is_codex_cli_installed();

        match (claude_cli, codex_cli) {
            (false, true) => AgentSelection {
                claude: false,
                codex: true,
                grok,
            },
            (true, false) => AgentSelection {
                claude: true,
                codex: false,
                grok,
            },
            // Keep existing preference when both are installed (or both absent).
            _ => AgentSelection {
                claude: true,
                codex: false,
                grok,
            },
        }
    } else {
        AgentSelection {
            claude,
            codex,
            grok,
        }
    }
}

// ============================================================================
// Confirmation and execution
// ============================================================================

fn confirm_and_apply(
    cas_dir: &Path,
    cwd: &Path,
    config: &WizardConfig,
    integration_flags: &super::integrate::integrations::IntegrationFlags,
) -> anyhow::Result<bool> {
    print_section_header("Confirmation")?;
    println!();

    // Calculate what files will be affected
    let cas_exists = cwd.join(".cas").exists();
    let settings_exists = cwd.join(".claude/settings.json").exists();
    let mcp_exists = cwd.join(".mcp.json").exists();
    let claude_md_exists = cwd.join("CLAUDE.md").exists();
    let skill_exists = cwd.join(".claude/skills/cas/SKILL.md").exists();
    let codex_config_exists = cwd.join(".codex/config.toml").exists();
    let codex_hooks_exists = cwd.join(".codex/hooks.json").exists();
    let gitignore_exists = cwd.join(".gitignore").exists();

    // Files to create
    print_colored("  Create:\n", colors::WHITE)?;

    if !cas_exists {
        print_file_item(".cas/", "CAS data directory", colors::GREEN)?;
    }
    print_file_item(".cas/config.toml", "Configuration", colors::GREEN)?;
    if !gitignore_exists {
        print_file_item(".gitignore", "Add .cas/ exclusion", colors::GREEN)?;
    }

    if config.agents.claude {
        if !mcp_exists {
            print_file_item(".mcp.json", "MCP server config", colors::GREEN)?;
        }
        if !settings_exists {
            print_file_item(".claude/settings.json", "Claude Code hooks", colors::GREEN)?;
        }
        if !skill_exists {
            print_file_item(".claude/skills/cas/SKILL.md", "CAS skill", colors::GREEN)?;
        }
        print_file_item(".claude/agents/", "Built-in agents", colors::GREEN)?;
        print_file_item(".claude/commands/", "Built-in commands", colors::GREEN)?;
    }

    if config.agents.codex {
        if !codex_config_exists {
            print_file_item(".codex/config.toml", "Codex MCP config", colors::GREEN)?;
        }
        if !codex_hooks_exists {
            print_file_item(
                ".codex/hooks.json",
                "CAS hook (review with /hooks)",
                colors::GREEN,
            )?;
        }
        print_file_item(".codex/agents/", "Built-in agents", colors::GREEN)?;
        print_file_item(".codex/commands/", "Built-in commands", colors::GREEN)?;
    }

    if config.agents.grok {
        if !mcp_exists {
            print_file_item(".mcp.json", "MCP server config (shared with Grok)", colors::GREEN)?;
        }
        print_file_item(".grok/agents/", "Built-in agents", colors::GREEN)?;
        print_file_item(".grok/skills/", "Built-in skills", colors::GREEN)?;
    }

    // Factory tooling files
    let env_template_exists = cwd.join(".env.worktree.template").exists();
    let boot_script_exists = cwd.join("scripts/worktree-boot.sh").exists();
    let has_factory_changes = !env_template_exists || !boot_script_exists;

    if has_factory_changes {
        println!();
        print_colored("  Factory tooling:\n", colors::WHITE)?;
        if !env_template_exists {
            print_file_item(
                ".env.worktree.template",
                "Worktree env template",
                colors::GREEN,
            )?;
        }
        if !boot_script_exists {
            print_file_item("scripts/worktree-boot.sh", "Boot script", colors::GREEN)?;
        }
    }

    // Files to modify
    let has_modifications = (config.agents.claude
        && (settings_exists || mcp_exists || claude_md_exists))
        || (config.agents.codex && (codex_config_exists || codex_hooks_exists))
        || (config.agents.grok && mcp_exists && !config.agents.claude)
        || gitignore_exists;
    if has_modifications {
        println!();
        print_colored("  Modify:\n", colors::WHITE)?;

        if gitignore_exists {
            print_file_item(".gitignore", "Add .cas/ exclusion", colors::ORANGE)?;
        }

        if config.agents.claude {
            if settings_exists {
                print_file_item(".claude/settings.json", "Add CAS hooks", colors::ORANGE)?;
            }
            if mcp_exists {
                print_file_item(".mcp.json", "Add CAS server", colors::ORANGE)?;
            }
            if claude_md_exists {
                print_file_item("CLAUDE.md", "Add CAS instructions", colors::ORANGE)?;
            } else {
                print_file_item("CLAUDE.md", "Create with CAS instructions", colors::GREEN)?;
            }
        }

        if config.agents.codex {
            if codex_config_exists {
                print_file_item(".codex/config.toml", "Add CAS server", colors::ORANGE)?;
            }
            if codex_hooks_exists {
                print_file_item(
                    ".codex/hooks.json",
                    "Install CAS hook (review with /hooks)",
                    colors::ORANGE,
                )?;
            }
        }

        // Only print .mcp.json's "Modify" row once — claude's clause above
        // already covers it when claude is also enabled.
        if config.agents.grok && mcp_exists && !config.agents.claude {
            print_file_item(".mcp.json", "Add CAS server (shared with Grok)", colors::ORANGE)?;
        }
    }

    println!();
    if !interactive::confirm("  Proceed", true)? {
        return Ok(false);
    }

    // Telemetry is opt-in; don't enable by default

    // Apply with animation
    apply_configuration(cas_dir, cwd, config, true, integration_flags)?;

    Ok(true)
}

fn print_file_item(path: &str, description: &str, color: Color) -> anyhow::Result<()> {
    print_colored("    ", colors::WHITE)?;
    print_colored(path, color)?;
    // Pad to align descriptions
    let padding = 32_usize.saturating_sub(path.len());
    print_colored(&" ".repeat(padding), colors::WHITE)?;
    print_colored(description, colors::GRAY)?;
    println!();
    Ok(())
}

// ============================================================================
// Animated execution
// ============================================================================

fn apply_configuration(
    cas_dir: &Path,
    cwd: &Path,
    config: &WizardConfig,
    animate: bool,
    integration_flags: &super::integrate::integrations::IntegrationFlags,
) -> anyhow::Result<()> {
    println!();

    // Step 1: Save configuration
    execute_step("Saving configuration", animate, || {
        let cas_config = config.to_config();
        cas_config.save(cas_dir)?;
        Ok(".cas/config.toml".to_string())
    })?;

    // Step 2: Ensure .cas is in .gitignore
    execute_step("Updating .gitignore", animate, || ensure_gitignore(cwd))?;

    if config.agents.claude {
        // Step 2: Configure local editor hooks
        execute_step("Configuring editor hooks", animate, || {
            configure_claude_hooks(cwd, false)?;
            Ok(".claude/settings.json".to_string())
        })?;

        // Step 3: Configure MCP server
        execute_step("Configuring MCP server", animate, || {
            configure_mcp_server(cwd)?;
            Ok(".mcp.json".to_string())
        })?;

        // Step 4: Update agent instructions
        execute_step("Updating agent instructions", animate, || {
            update_claude_md(cwd)?;
            Ok("CLAUDE.md".to_string())
        })?;

        // Step 5: Generate CAS skill
        execute_step("Generating CAS guidance skill", animate, || {
            generate_cas_skill(cwd)?;
            Ok(".claude/skills/cas/SKILL.md".to_string())
        })?;

        // Step 6: Sync built-ins
        execute_step("Syncing built-in files", animate, || {
            let claude_dir = cwd.join(".claude");
            let result =
                sync_all_builtins_for_harness(cas_mux::SupervisorCli::Claude, &claude_dir)?;
            let total = result.agents_updated + result.skills_updated;
            Ok(format!("{total} files"))
        })?;
    }

    if config.agents.codex {
        execute_step("Configuring Codex MCP server", animate, || {
            configure_codex_mcp_server(cwd)?;
            Ok(".codex/config.toml + .codex/hooks.json; review with /hooks".to_string())
        })?;

        execute_step("Syncing Codex built-in files", animate, || {
            let codex_dir = cwd.join(".codex");
            let result = sync_all_builtins_for_harness(cas_mux::SupervisorCli::Codex, &codex_dir)?;
            let total = result.agents_updated + result.skills_updated;
            Ok(format!("{total} files"))
        })?;
    }

    if config.agents.grok {
        // Grok reads `.mcp.json` directly (verified via `grok mcp doctor`) —
        // reuse the same idempotent writer Claude uses instead of a
        // separate Grok-specific config file.
        execute_step("Configuring MCP server for Grok", animate, || {
            configure_mcp_server(cwd)?;
            Ok(".mcp.json".to_string())
        })?;

        execute_step("Syncing Grok built-in files", animate, || {
            let grok_dir = cwd.join(".grok");
            let result = sync_all_builtins_for_harness(cas_mux::SupervisorCli::Grok, &grok_dir)?;
            let total = result.agents_updated + result.skills_updated;
            Ok(format!("{total} files"))
        })?;
    }

    // Step 7: Setup factory tooling helper templates
    execute_step("Setting up factory tooling", animate, || {
        factory_tooling::setup_factory_tooling(cwd)
    })?;

    // Step 8 (cas-7417): Vercel/Neon/GitHub auto-integration.
    // Run after factory tooling so the project is fully bootstrapped before
    // we touch platform-specific skills. Uses the orchestration layer in
    // `cli/integrate/integrations.rs` which acquires the integrate lockfile,
    // detects each platform, and dispatches to the corresponding handler.
    let ux = if animate {
        super::integrate::integrations::UxMode::Interactive
    } else {
        super::integrate::integrations::UxMode::NonInteractive
    };
    match super::integrate::integrations::run(cwd, integration_flags, ux) {
        Ok(report) => super::integrate::integrations::render(&report),
        Err(e) => {
            // Don't fail the entire init if integrations explode — they're
            // additive. Surface the error and continue.
            print_colored("  ! ", colors::ORANGE)?;
            print_colored(
                &format!("Integrations failed: {e:#}\n"),
                colors::WHITE,
            )?;
        }
    }

    // Final success message
    println!();
    print_colored("  ✓ ", colors::GREEN)?;
    print_colored("CAS initialized at ", colors::WHITE)?;
    print_colored(&cas_dir.display().to_string(), colors::CYAN)?;
    println!("\n");

    Ok(())
}

fn execute_step<F>(label: &str, animate: bool, action: F) -> anyhow::Result<()>
where
    F: FnOnce() -> anyhow::Result<String>,
{
    let mut stdout = stdout();
    let started = Instant::now();
    info!(phase = label, "cas init: phase starting");

    // Show spinner
    print_colored("  ", colors::WHITE)?;
    print_colored(&format!("{}", SPINNER_FRAMES[0]), colors::ORANGE)?;
    print_colored(&format!(" {label}..."), colors::WHITE)?;
    stdout.flush()?;

    if animate {
        // Animate spinner briefly
        for i in 0..8 {
            thread::sleep(Duration::from_millis(50));
            print!("\r");
            print_colored("  ", colors::WHITE)?;
            print_colored(
                &format!("{}", SPINNER_FRAMES[i % SPINNER_FRAMES.len()]),
                colors::ORANGE,
            )?;
            print_colored(&format!(" {label}..."), colors::WHITE)?;
            stdout.flush()?;
        }
    }

    // Execute action
    match action() {
        Ok(result) => {
            // Show success
            print!("\r");
            print_colored("  ✓ ", colors::GREEN)?;
            print_colored(label, colors::WHITE)?;
            // Pad to clear any remnants
            let padding = 40_usize.saturating_sub(label.len());
            print_colored(&" ".repeat(padding), colors::WHITE)?;
            print_colored(&result, colors::GRAY)?;
            println!();
            info!(
                phase = label,
                elapsed_ms = started.elapsed().as_millis() as u64,
                detail = %result,
                "cas init: phase completed"
            );
            Ok(())
        }
        Err(e) => {
            // Show failure
            print!("\r");
            print_colored("  ✗ ", colors::RED)?;
            print_colored(label, colors::WHITE)?;
            print_colored(" — ", colors::GRAY)?;
            print_colored(&format!("{e}"), colors::RED)?;
            println!();
            error!(
                phase = label,
                elapsed_ms = started.elapsed().as_millis() as u64,
                error = %e,
                "cas init: phase failed"
            );
            Err(e)
        }
    }
}

// ============================================================================
// Quick start guide
// ============================================================================

fn print_quick_start() {
    use crate::ui::components::{Formatter, Renderable, Table};
    use crate::ui::theme::ActiveTheme;

    let table = Table::new()
        .columns(&["Command", "Description"])
        .rows(vec![
            vec!["cas", "Launch multi-agent factory"],
            vec!["cas attach", "Attach to running session"],
            vec!["cas serve", "Start MCP server"],
            vec!["cas doctor", "Run diagnostics"],
        ])
        .indent(2);

    let theme = ActiveTheme::default();
    let mut out = std::io::stdout();
    let mut fmt = Formatter::stdout(&mut out, theme);
    let _ = fmt.newline();
    let _ = table.render(&mut fmt);
    let _ = fmt.newline();
}

// ============================================================================
// Post-init next steps
// ============================================================================

/// Check if .claude/settings.json is tracked by git (committed at least once).
fn is_claude_dir_tracked(cwd: &Path) -> bool {
    Command::new("git")
        .args(["ls-files", "--error-unmatch", ".claude/settings.json"])
        .current_dir(cwd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Check if we're inside a git repository.
fn is_git_repo(cwd: &Path) -> bool {
    Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(cwd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Print "Next steps" box telling users to commit .claude/ for factory workers.
/// Skips if not in a git repo or if .claude/ is already tracked.
fn print_next_steps(cwd: &Path) {
    if !is_git_repo(cwd) || is_claude_dir_tracked(cwd) {
        return;
    }

    let _ = (|| -> anyhow::Result<()> {
        print_colored(
            "  ┌─ Next steps ──────────────────────────────────────────┐\n",
            colors::CYAN,
        )?;
        print_colored("  │", colors::CYAN)?;
        print_colored(
            "                                                        ",
            colors::WHITE,
        )?;
        print_colored("│\n", colors::CYAN)?;

        print_colored("  │", colors::CYAN)?;
        print_colored(
            "  Commit CAS config so factory workers can access it:   ",
            colors::WHITE,
        )?;
        print_colored("│\n", colors::CYAN)?;

        print_colored("  │", colors::CYAN)?;
        print_colored(
            "                                                        ",
            colors::WHITE,
        )?;
        print_colored("│\n", colors::CYAN)?;

        print_colored("  │", colors::CYAN)?;
        print_colored(
            "    git add .claude/ CLAUDE.md .mcp.json .gitignore     ",
            colors::GREEN,
        )?;
        print_colored("│\n", colors::CYAN)?;

        print_colored("  │", colors::CYAN)?;
        print_colored(
            "    git commit -m \"Configure CAS\"                       ",
            colors::GREEN,
        )?;
        print_colored("│\n", colors::CYAN)?;

        print_colored("  │", colors::CYAN)?;
        print_colored(
            "                                                        ",
            colors::WHITE,
        )?;
        print_colored("│\n", colors::CYAN)?;

        print_colored(
            "  └────────────────────────────────────────────────────────┘\n",
            colors::CYAN,
        )?;
        println!();
        Ok(())
    })();
}

/// Returns next steps as JSON-friendly data (for --json mode).
fn next_steps_needed(cwd: &Path) -> Option<Vec<String>> {
    if !is_git_repo(cwd) || is_claude_dir_tracked(cwd) {
        return None;
    }
    Some(vec![
        "git add .claude/ CLAUDE.md .mcp.json .gitignore".to_string(),
        "git commit -m \"Configure CAS\"".to_string(),
    ])
}

// ============================================================================
// Helper functions
// ============================================================================

fn print_colored(text: &str, color: Color) -> anyhow::Result<()> {
    let mut stdout = stdout();
    execute!(stdout, SetForegroundColor(color), Print(text))?;
    execute!(stdout, SetForegroundColor(Color::Reset))?;
    Ok(())
}

// ============================================================================
// .gitignore management
// ============================================================================

/// Ensure `.cas` is listed in `.gitignore`. If no `.gitignore` exists, create one.
fn ensure_gitignore(cwd: &Path) -> anyhow::Result<String> {
    let gitignore_path = cwd.join(".gitignore");
    if gitignore_path.exists() {
        let content = std::fs::read_to_string(&gitignore_path)?;
        // Check if .cas is already ignored (exact line match)
        let already_ignored = content.lines().any(|line| {
            let trimmed = line.trim();
            trimmed == ".cas" || trimmed == ".cas/" || trimmed == "/.cas" || trimmed == "/.cas/"
        });
        if already_ignored {
            return Ok("already in .gitignore".to_string());
        }
        // Append .cas to existing .gitignore
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&gitignore_path)?;
        // Ensure we start on a new line
        if !content.ends_with('\n') {
            std::io::Write::write_all(&mut file, b"\n")?;
        }
        std::io::Write::write_all(&mut file, b".cas/\n")?;
        Ok(".gitignore (appended)".to_string())
    } else {
        std::fs::write(&gitignore_path, ".cas/\n")?;
        Ok(".gitignore (created)".to_string())
    }
}

// ============================================================================
// CLAUDE.md management
// ============================================================================

/// Marker for CAS-managed section in CLAUDE.md
mod docs_and_skill;

pub(crate) use crate::cli::init::docs_and_skill::{
    CAS_SECTION_BEGIN, CAS_SECTION_END, CAS_SKILL, build_cas_section, is_old_cas_skill,
    is_skill_managed_by_cas,
};
pub use crate::cli::init::docs_and_skill::{generate_cas_skill, update_claude_md};

#[cfg(test)]
mod integration_flag_tests {
    use super::*;

    #[test]
    fn integration_flags_from_threads_each_field() {
        let args = InitArgs {
            yes: true,
            force: false,
            no_integrations: true,
            vercel: Some("prj_abc".to_string()),
            neon: Some("np_xyz".to_string()),
            github: Some("acme/widgets".to_string()),
        };
        let flags = integration_flags_from(&args);
        assert!(flags.disabled);
        assert_eq!(flags.vercel_project.as_deref(), Some("prj_abc"));
        assert_eq!(flags.neon_project.as_deref(), Some("np_xyz"));
        assert_eq!(flags.github_repo.as_deref(), Some("acme/widgets"));
    }

    #[test]
    fn integration_flags_from_defaults_when_unset() {
        let args = InitArgs::default();
        let flags = integration_flags_from(&args);
        assert!(!flags.disabled);
        assert!(flags.vercel_project.is_none());
        assert!(flags.neon_project.is_none());
        assert!(flags.github_repo.is_none());
    }
}

/// EPIC cas-8888 (cas-6f46, Phase 5): Grok config wiring — the
/// `agents.grok` toggle and its detection.
#[cfg(test)]
mod grok_agent_selection_tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn default_wizard_config_grok_is_false() {
        let config = WizardConfig::default();
        assert!(!config.agents.grok);
        // Regression guard: adding grok must not perturb the existing
        // claude/codex defaults.
        assert!(config.agents.claude);
        assert!(!config.agents.codex);
    }

    #[test]
    fn detect_agent_defaults_grok_false_on_fresh_project() {
        let temp = TempDir::new().unwrap();
        let selection = detect_agent_defaults(temp.path());
        assert!(
            !selection.grok,
            "grok is opt-in only — a fresh project must never default to grok=true"
        );
    }

    #[test]
    fn detect_agent_defaults_grok_true_when_grok_dir_exists() {
        let temp = TempDir::new().unwrap();
        std::fs::create_dir_all(temp.path().join(".grok")).unwrap();
        // Need at least one of claude/.codex to exist too, or the fresh-project
        // branch runs instead of the existing-project branch.
        std::fs::create_dir_all(temp.path().join(".claude")).unwrap();

        let selection = detect_agent_defaults(temp.path());
        assert!(
            selection.grok,
            "an existing .grok/ dir means a prior init/update already opted in"
        );
    }

    #[test]
    fn detect_agent_defaults_grok_false_when_grok_dir_absent_but_claude_present() {
        let temp = TempDir::new().unwrap();
        std::fs::create_dir_all(temp.path().join(".claude")).unwrap();

        let selection = detect_agent_defaults(temp.path());
        assert!(!selection.grok);
        assert!(selection.claude);
    }
}
