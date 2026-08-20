//! Cassy - Coding Agent System
//!
//! A CLI tool for AI agents to build persistent memory across sessions.

use std::process::ExitCode;

// Use the library crate
use cas::{cli, config, duplicate_check, error, logging, sentry, store};

fn main() -> ExitCode {
    // Reset SIGPIPE to default behavior so broken pipes cause a clean exit
    // instead of panicking with "failed printing to stderr: Broken pipe"
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    // Install rustls' default CryptoProvider before anything touches TLS.
    // tokio-tungstenite 0.24 + rustls 0.23 otherwise panic inside the daemon's
    // cloud-client worker with "Could not automatically determine the
    // process-level CryptoProvider from Rustls crate features", which crashes
    // the factory daemon after it has already bound its unix socket — leaving
    // clients with a confusing "Connection refused" error.
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Initialize Sentry crash reporting (respects telemetry config)
    let _sentry_guard = sentry::init(sentry::is_sentry_enabled());

    let raw_args: Vec<String> = std::env::args().collect();
    let parse_args = maybe_rewrite_factory_args(raw_args);

    // Warn if multiple `cas` binaries on PATH have diverging mtimes. Best-effort,
    // gated to interactive terminals + non-silent subcommands. Runs AFTER the
    // factory-alias rewrite so `cas --new -w4` (which is rewritten to
    // `cas factory --new -w4`) is recognized as the factory subcommand and
    // stays silent. See `duplicate_check::should_run`.
    duplicate_check::check_and_warn(&parse_args);

    // Use try_parse to capture argument errors for tracing
    let cli = match cli::try_parse_from_with_wordmark(parse_args) {
        Ok(cli) => cli,
        Err(e) => {
            // Record the argument parsing error
            record_arg_error(&e);

            // Let clap handle the error display (includes help text)
            e.exit();
        }
    };

    // Initialize logging (after CLI parsing so we can use --verbose)
    init_logging(&cli);

    // Add breadcrumb for crash context (command being executed)
    let args = redact_viktor_key_args(std::env::args().skip(1).collect());
    sentry::add_command_breadcrumb(&args.join(" "));

    match cli::run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            use cas::ui::components::Formatter;
            use cas::ui::theme::ActiveTheme;

            let mut err_out = std::io::stderr();
            let theme = ActiveTheme::default();
            let mut fmt = Formatter::stdout(&mut err_out, theme);

            if let Some(err) = e.downcast_ref::<error::CasError>() {
                let _ = fmt.error(&err.to_string());

                // Show suggestion if available
                if let Some(suggestion) = err.suggestion() {
                    let _ = fmt.newline();
                    let _ = fmt.warning("Suggestion:");
                    let _ = fmt.indent_block(suggestion);
                }

                // Return specific exit codes for different error types
                match err {
                    error::CasError::NotFound(_)
                    | error::CasError::EntryNotFound(_)
                    | error::CasError::RuleNotFound(_)
                    | error::CasError::TaskNotFound(_)
                    | error::CasError::SkillNotFound(_) => ExitCode::from(2),
                    error::CasError::NotInitialized => ExitCode::from(3),
                    error::CasError::Parse(_)
                    | error::CasError::InvalidEntryType(_)
                    | error::CasError::InvalidRuleStatus(_)
                    | error::CasError::InvalidTaskStatus(_) => ExitCode::from(4),
                    _ => ExitCode::FAILURE,
                }
            } else {
                let _ = fmt.error(&format!("{e:#}"));
                ExitCode::FAILURE
            }
        }
    }
}

fn maybe_rewrite_factory_args(args: Vec<String>) -> Vec<String> {
    if args.len() <= 1 {
        return args;
    }

    // Respect explicit subcommands; only infer "factory" for option-only invocations.
    if has_explicit_subcommand(&args[1..]) {
        return args;
    }
    if !contains_factory_launch_flag(&args[1..]) {
        return args;
    }

    let mut rewritten = Vec::with_capacity(args.len() + 1);
    rewritten.push(args[0].clone());
    rewritten.push("factory".to_string());
    rewritten.extend(args.iter().skip(1).cloned());
    rewritten
}

fn has_explicit_subcommand(tokens: &[String]) -> bool {
    let mut i = 0usize;
    while i < tokens.len() {
        let token = &tokens[i];
        if token == "--" {
            return i + 1 < tokens.len();
        }

        if let Some(consumes_next) = flag_consumes_value(token) {
            if consumes_next {
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }

        if token.starts_with('-') {
            i += 1;
            continue;
        }

        return true;
    }
    false
}

/// Returns Some(true) if this flag consumes the next token as a value,
/// Some(false) for known no-value flags, or None if unknown.
fn flag_consumes_value(token: &str) -> Option<bool> {
    // Global flags (no values)
    if matches!(
        token,
        "--json" | "--full" | "--verbose" | "-v" | "--help" | "-h" | "--version" | "-V"
    ) {
        return Some(false);
    }

    // Factory flags that consume a value.
    if matches!(
        token,
        "--workers"
            | "-w"
            | "--name"
            | "-n"
            | "--worktree-root"
            | "--supervisor-cli"
            | "--worker-cli"
    ) {
        return Some(true);
    }
    if token.starts_with("--workers=")
        || token.starts_with("--name=")
        || token.starts_with("--worktree-root=")
        || token.starts_with("--supervisor-cli=")
        || token.starts_with("--worker-cli=")
    {
        return Some(false);
    }
    if token.starts_with("-w") || token.starts_with("-n") {
        // Short form with inline value, e.g. -w2 or -nmy-session
        return Some(false);
    }

    // Known no-value factory flags
    if matches!(
        token,
        "--new"
            | "--no-worktrees"
            | "--cleanup"
            | "--dry-run"
            | "--force"
            | "-f"
            | "--notify"
            | "--bell"
            | "--tabbed"
            | "--record"
            | "--legacy"
    ) {
        return Some(false);
    }

    None
}

fn contains_factory_launch_flag(tokens: &[String]) -> bool {
    tokens.iter().any(|token| {
        matches!(
            token.as_str(),
            "--new"
                | "--workers"
                | "-w"
                | "--name"
                | "-n"
                | "--no-worktrees"
                | "--worktree-root"
                | "--cleanup"
                | "--dry-run"
                | "--force"
                | "-f"
                | "--notify"
                | "--bell"
                | "--tabbed"
                | "--record"
                | "--supervisor-cli"
                | "--worker-cli"
                | "--legacy"
        ) || token.starts_with("--workers=")
            || token.starts_with("--name=")
            || token.starts_with("--worktree-root=")
            || token.starts_with("--supervisor-cli=")
            || token.starts_with("--worker-cli=")
            || (token.starts_with("-w") && token.len() > 2)
            || (token.starts_with("-n") && token.len() > 2)
    })
}

/// Initialize the logging system
fn init_logging(cli: &cli::Cli) {
    // Try to find Cassy root and load config
    let cas_root = store::find_cas_root().ok();
    let logging_config = cas_root
        .as_ref()
        .and_then(|root| config::Config::load(root).ok())
        .and_then(|c| c.logging)
        .unwrap_or_default();

    // Initialize logging with config
    if let Err(e) = logging::init(cas_root.as_deref(), cli.verbose, &logging_config) {
        // Don't fail the command, just warn if verbose
        if cli.verbose {
            eprintln!("Warning: Failed to initialize logging: {e}");
        }
    }

    // Clean up old logs on startup (best-effort)
    if let Some(root) = &cas_root {
        let log_dir = root.join(&logging_config.log_dir);
        if log_dir.exists() {
            let _ = logging::cleanup_old_logs(&log_dir, logging_config.retention_days);
        }

        // Clean up obsolete vector/embedding files (semantic search removed)
        cleanup_vector_files(root);
    }
}

/// Clean up obsolete vector and embedding files from previous versions.
/// Semantic search has been removed, so these files are no longer needed.
fn cleanup_vector_files(cas_root: &std::path::Path) {
    use std::fs;
    use tracing::debug;

    // Files to delete
    let files_to_delete = ["vectors.hnsw", "vectors.meta.json"];

    for filename in &files_to_delete {
        let path = cas_root.join(filename);
        if path.exists() {
            match fs::remove_file(&path) {
                Ok(()) => debug!("Removed obsolete file: {}", path.display()),
                Err(e) => debug!("Failed to remove {}: {}", path.display(), e),
            }
        }
    }

    // Delete models directory
    let models_dir = cas_root.join("models");
    if models_dir.exists() && models_dir.is_dir() {
        match fs::remove_dir_all(&models_dir) {
            Ok(()) => debug!(
                "Removed obsolete models directory: {}",
                models_dir.display()
            ),
            Err(e) => debug!("Failed to remove models directory: {}", e),
        }
    }
}

/// Record argument parsing errors for tracing bad Cassy usage
fn record_arg_error(err: &clap::Error) {
    use cas::tracing::{DevTracer, TraceTimer};

    // Skip tracing for --help and --version (these are not actual errors)
    if matches!(
        err.kind(),
        clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
    ) {
        return;
    }

    // Try to initialize tracer (may fail if not in a Cassy project)
    if let Ok(cas_root) = store::find_cas_root() {
        if DevTracer::init_global(&cas_root).unwrap_or(false) {
            if let Some(tracer) = DevTracer::get() {
                // Only trace if command tracing is enabled
                if tracer.should_trace_commands() {
                    let timer = TraceTimer::new();

                    // Extract the raw args that were passed
                    let args = redact_viktor_key_args(std::env::args().skip(1).collect());

                    // Determine error kind for categorization
                    let error_kind = match err.kind() {
                        clap::error::ErrorKind::InvalidValue => "invalid_value",
                        clap::error::ErrorKind::UnknownArgument => "unknown_argument",
                        clap::error::ErrorKind::InvalidSubcommand => "invalid_subcommand",
                        clap::error::ErrorKind::MissingRequiredArgument => "missing_required",
                        clap::error::ErrorKind::MissingSubcommand => "missing_subcommand",
                        clap::error::ErrorKind::WrongNumberOfValues => "wrong_value_count",
                        clap::error::ErrorKind::ValueValidation => "value_validation",
                        _ => "other",
                    };

                    let _ = tracer.record_command(
                        &format!("parse_error:{error_kind}"),
                        &args,
                        timer.elapsed_ms(),
                        false,
                        Some(&err.to_string()),
                    );
                }
            }
        }
    }
}

/// Keep the only supported operator key-entry command out of crash and
/// development telemetry, including parse-error diagnostics.
fn redact_viktor_key_args(mut args: Vec<String>) -> Vec<String> {
    if let Some(index) = args
        .windows(2)
        .position(|window| window[0] == "viktor" && window[1] == "key")
        && let Some(key) = args.get_mut(index + 2)
    {
        *key = "[REDACTED]".to_string();
    }
    args
}

#[cfg(test)]
mod tests {
    use super::redact_viktor_key_args;

    #[test]
    fn viktor_key_is_redacted_from_telemetry_arguments() {
        assert_eq!(
            redact_viktor_key_args(vec![
                "--json".to_string(),
                "viktor".to_string(),
                "key".to_string(),
                "operator-issued-secret".to_string(),
            ]),
            vec!["--json", "viktor", "key", "[REDACTED]"]
        );
    }
}
