use std::collections::HashMap;
use std::path::Path;

use crate::cli::Cli;
use crate::cli::config::ConfigEditArgs;
use crate::config::{Config, registry};
use crate::ui::components::Formatter;
use crate::ui::theme::ActiveTheme;

use crate::cli::config::util::truncate_description;

pub(crate) fn execute_edit(
    args: &ConfigEditArgs,
    cli: &Cli,
    cas_root: &Path,
) -> anyhow::Result<()> {
    use std::io::IsTerminal;

    // Use TUI if we're in a terminal, otherwise fall back to line-based editing
    if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() && !cli.json {
        // Use the full TUI editor
        crate::cli::config_tui::run_tui(args.section.clone(), cas_root)
    } else {
        // Fall back to line-based editing (for non-TTY or JSON mode)
        execute_edit_line_based(args, cli, cas_root)
    }
}

pub(crate) fn execute_edit_line_based(
    args: &ConfigEditArgs,
    cli: &Cli,
    cas_root: &Path,
) -> anyhow::Result<()> {
    use std::io::{self, BufRead, Write};

    let mut config = Config::load(cas_root)?;
    let original = config.clone();
    let reg = registry();

    // Get all current values
    let items: HashMap<String, String> = config.list().into_iter().collect();

    // Collect configs to edit
    let mut configs_to_edit: Vec<&crate::config::ConfigMeta> = Vec::new();

    for section in reg.sections() {
        if let Some(filter_section) = &args.section {
            if !section.starts_with(filter_section.as_str()) {
                continue;
            }
        }

        for meta in reg.configs_in_section(section) {
            if !args.all && meta.advanced {
                continue;
            }

            if args.modified {
                let value = items
                    .get(meta.key)
                    .cloned()
                    .unwrap_or_else(|| meta.default.to_string());
                if !meta.is_modified(&value) {
                    continue;
                }
            }

            configs_to_edit.push(meta);
        }
    }

    let theme = ActiveTheme::default();
    let mut out = std::io::stdout();
    let mut fmt = Formatter::stdout(&mut out, theme);

    if configs_to_edit.is_empty() {
        fmt.warning("No config options to edit")?;
        return Ok(());
    }

    fmt.heading("Interactive Config Editor")?;
    fmt.newline()?;
    fmt.write_raw("For each option, enter a new value or:")?;
    fmt.newline()?;
    fmt.key_hint("Enter", "keep current value")?;
    fmt.key_hint("d", "reset to default")?;
    fmt.key_hint("q", "skip remaining")?;
    fmt.newline()?;

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut changes_made = 0;

    for meta in configs_to_edit {
        let current_value = items
            .get(meta.key)
            .cloned()
            .unwrap_or_else(|| meta.default.to_string());
        let is_modified = meta.is_modified(&current_value);

        // Display the option
        fmt.write_accent(&format!("## {}", meta.name))?;
        fmt.newline()?;
        fmt.write_raw("   ")?;
        let warning_color = fmt.theme().palette.status_warning;
        fmt.write_colored(meta.key, warning_color)?;
        fmt.newline()?;
        fmt.write_raw("   ")?;
        fmt.write_muted(&truncate_description(meta.description, 70))?;
        fmt.newline()?;
        fmt.write_raw(&format!(
            "   Type: {} | Default: {}\n",
            meta.value_type.name(),
            meta.default,
        ))?;

        let current_display = if is_modified {
            format!("{current_value} (modified)")
        } else {
            current_value.clone()
        };
        print!("   Current: {current_display} > ");
        stdout.flush()?;

        // Read input
        let mut input = String::new();
        stdin.lock().read_line(&mut input)?;
        let input = input.trim();

        match input {
            "" => {
                // Keep current value
                fmt.write_raw("   ")?;
                fmt.write_muted("Kept")?;
                fmt.newline()?;
            }
            "q" | "Q" => {
                fmt.newline()?;
                fmt.warning("Stopped editing")?;
                break;
            }
            "d" | "D" => {
                // Reset to default
                if current_value != meta.default {
                    config.set(meta.key, meta.default)?;
                    changes_made += 1;
                    fmt.success(&format!("   Reset to: {}", meta.default))?;
                } else {
                    fmt.write_raw("   ")?;
                    fmt.write_muted("Already at default")?;
                    fmt.newline()?;
                }
            }
            new_value => {
                // Validate and set
                match meta.validate(new_value) {
                    Ok(()) => {
                        if new_value != current_value {
                            config.set(meta.key, new_value)?;
                            changes_made += 1;
                            fmt.success(&format!("   Set to: {new_value}"))?;
                        } else {
                            fmt.write_raw("   ")?;
                            fmt.write_muted("Unchanged")?;
                            fmt.newline()?;
                        }
                    }
                    Err(e) => {
                        fmt.error(&format!("   Invalid: {e}"))?;
                        fmt.write_raw("   ")?;
                        fmt.write_muted("Kept previous value")?;
                        fmt.newline()?;
                    }
                }
            }
        }
        fmt.newline()?;
    }

    // Save if changes were made
    if changes_made > 0 {
        // cas-0d4f0: piped edits may not relax a security-relevant key.
        crate::cli::config::operator_ops::guard_operator_config(
            &original,
            &config,
            crate::config::operator_policy::InvocationContext::from_process,
        )?;
        config.save(cas_root)?;
        fmt.success(&format!("Saved {changes_made} change(s)"))?;
    } else {
        fmt.write_muted("No changes made")?;
        fmt.newline()?;
    }

    if cli.json {
        println!("{}", serde_json::json!({ "changes": changes_made }));
    }

    Ok(())
}
