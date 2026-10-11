use std::collections::HashMap;
use std::io::{self, Write};
use std::path::Path;

use crate::cli::Cli;
use crate::cli::config::{ConfigExportArgs, ConfigImportArgs};
use crate::config::Config;
use crate::ui::components::{Formatter, Renderable, StatusLine};
use crate::ui::theme::ActiveTheme;

pub(crate) fn execute_export(
    args: &ConfigExportArgs,
    cli: &Cli,
    cas_root: &Path,
) -> anyhow::Result<()> {
    let config = Config::load(cas_root)?;

    let content = match args.format.as_str() {
        "yaml" | "yml" => serde_yaml::to_string(&config)?,
        "json" => serde_json::to_string_pretty(&config)?,
        _ => {
            let theme = ActiveTheme::default();
            let mut stderr = io::stderr();
            let mut fmt = Formatter::stdout(&mut stderr, theme);
            StatusLine::error(format!(
                "Unknown format: {}. Use 'yaml' or 'json'",
                args.format
            ))
            .render(&mut fmt)?;
            std::process::exit(1);
        }
    };

    if let Some(output) = &args.output {
        std::fs::write(output, &content)?;
        if !cli.json {
            let theme = ActiveTheme::default();
            let mut stdout = io::stdout();
            let mut fmt = Formatter::stdout(&mut stdout, theme);
            StatusLine::success(format!("Exported configuration to {output}")).render(&mut fmt)?;
        } else {
            println!(r#"{{"exported":"{}","format":"{}"}}"#, output, args.format);
        }
    } else {
        let mut stdout = io::stdout();
        writeln!(stdout, "{content}")?;
    }

    Ok(())
}

pub(crate) fn execute_import(
    args: &ConfigImportArgs,
    cli: &Cli,
    cas_root: &Path,
) -> anyhow::Result<()> {
    let current_config = Config::load(cas_root)?;

    let content = std::fs::read_to_string(&args.file)?;

    // Try to parse as YAML first, then JSON
    let imported_config: Config = serde_yaml::from_str(&content)
        .or_else(|_| serde_json::from_str(&content))
        .map_err(|e| anyhow::anyhow!("Failed to parse config file: {e}"))?;

    if args.dry_run {
        // Show what would change
        let current_items: HashMap<String, String> = current_config.list().into_iter().collect();
        let imported_items: HashMap<String, String> = imported_config.list().into_iter().collect();

        let mut changes = Vec::new();
        for (key, new_value) in &imported_items {
            let old_value = current_items.get(key).cloned().unwrap_or_default();
            if old_value != *new_value {
                changes.push((key.clone(), old_value, new_value.clone()));
            }
        }

        if cli.json {
            let json: Vec<_> = changes
                .iter()
                .map(|(k, old, new)| {
                    serde_json::json!({
                        "key": k,
                        "old_value": old,
                        "new_value": new
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&json)?);
        } else {
            let theme = ActiveTheme::default();
            let mut stdout = io::stdout();
            let mut fmt = Formatter::stdout(&mut stdout, theme);

            if changes.is_empty() {
                StatusLine::info("No changes would be made").render(&mut fmt)?;
            } else {
                StatusLine::info(format!("Would make {} changes:", changes.len()))
                    .render(&mut fmt)?;
                for (key, old, new) in &changes {
                    fmt.bullet(&format!("{key} = {old} -> {new}"))?;
                }
            }
        }
    } else {
        if !args.force {
            let theme = ActiveTheme::default();
            let mut stderr = io::stderr();
            let mut fmt = Formatter::stdout(&mut stderr, theme);
            StatusLine::warning("This will overwrite your current configuration.")
                .render(&mut fmt)?;
            StatusLine::info("Use --force to confirm, or --dry-run to preview changes.")
                .render(&mut fmt)?;
            std::process::exit(1);
        }

        // cas-0d4f0: an import may not relax a security-relevant key.
        crate::cli::config::operator_ops::guard_operator_config(
            &current_config,
            &imported_config,
            crate::config::operator_policy::InvocationContext::from_process,
        )?;
        imported_config.save(cas_root)?;

        if cli.json {
            println!(r#"{{"imported":"{}"}}"#, args.file);
        } else {
            let theme = ActiveTheme::default();
            let mut stdout = io::stdout();
            let mut fmt = Formatter::stdout(&mut stdout, theme);
            StatusLine::success(format!("Imported configuration from {}", args.file))
                .render(&mut fmt)?;
        }
    }

    Ok(())
}
