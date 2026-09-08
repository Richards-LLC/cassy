use std::collections::HashMap;
use std::path::Path;

use crate::cli::Cli;
use crate::cli::config::{
    ConfigDescribeArgs, ConfigDiffArgs, ConfigGetArgs, ConfigListArgs, ConfigResetArgs,
    ConfigSearchArgs, ConfigSetArgs,
};
use crate::config::{Config, registry};
use crate::ui::components::Formatter;
use crate::ui::theme::ActiveTheme;

use crate::cli::config::util::{format_constraint, truncate_description};

pub(crate) fn execute_get(args: &ConfigGetArgs, cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    let config = Config::load(cas_root)?;
    execute_get_internal(&config, &args.key, cli)
}

fn execute_get_internal(config: &Config, key: &str, cli: &Cli) -> anyhow::Result<()> {
    if let Some(value) = config.get(key) {
        if cli.json {
            let meta = registry().get(key);
            let json = serde_json::json!({
                "key": key,
                "value": value,
                "type": meta.map(|m| m.value_type.name()),
                "default": meta.map(|m| m.default),
                "modified": meta.map(|m| m.is_modified(&value)).unwrap_or(false)
            });
            println!("{}", serde_json::to_string_pretty(&json)?);
        } else {
            println!("{value}");
        }
        Ok(())
    } else {
        if cli.json {
            println!(r#"{{"error":"unknown key","key":"{key}"}}"#);
        } else {
            eprintln!("Unknown config key: {key}");

            // Suggest similar keys
            let suggestions = registry().search(key);
            if !suggestions.is_empty() {
                eprintln!("\nDid you mean:");
                for meta in suggestions.iter().take(3) {
                    eprintln!("  {}", meta.key);
                }
            }
        }
        std::process::exit(1);
    }
}

pub(crate) fn execute_set(args: &ConfigSetArgs, cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    let mut config = Config::load(cas_root)?;
    execute_set_internal(&mut config, &args.key, &args.value, cas_root, cli)
}

fn execute_set_internal(
    config: &mut Config,
    key: &str,
    value: &str,
    cas_root: &std::path::Path,
    cli: &Cli,
) -> anyhow::Result<()> {
    let old_value = config.get(key).unwrap_or_default();

    config.set(key, value)?;
    config.save(cas_root)?;

    if cli.json {
        let json = serde_json::json!({
            "key": key,
            "old_value": old_value,
            "new_value": value
        });
        println!("{}", serde_json::to_string(&json)?);
    } else {
        let theme = ActiveTheme::default();
        let mut out = std::io::stdout();
        let mut fmt = Formatter::stdout(&mut out, theme);
        fmt.success(&format!("Set {key} = {value}"))?;
        if !old_value.is_empty() && old_value != value {
            fmt.write_raw("  ")?;
            fmt.write_muted(&format!("was: {old_value}"))?;
            fmt.newline()?;
        }
    }
    Ok(())
}

pub(crate) fn execute_list(
    args: &ConfigListArgs,
    cli: &Cli,
    cas_root: &Path,
) -> anyhow::Result<()> {
    let config = Config::load(cas_root)?;
    let reg = registry();

    // Get all current values
    let items: HashMap<String, String> = config.list().into_iter().collect();

    if cli.json {
        let mut result = serde_json::Map::new();

        for section in reg.sections() {
            if let Some(filter_section) = &args.section {
                if !section.starts_with(filter_section.as_str()) {
                    continue;
                }
            }

            let mut section_items = serde_json::Map::new();
            for meta in reg.configs_in_section(section) {
                if !args.all && meta.advanced {
                    continue;
                }

                let value = items
                    .get(meta.key)
                    .cloned()
                    .unwrap_or_else(|| meta.default.to_string());
                let is_modified = meta.is_modified(&value);

                if args.modified && !is_modified {
                    continue;
                }

                section_items.insert(
                    meta.key.to_string(),
                    serde_json::json!({
                        "value": value,
                        "default": meta.default,
                        "type": meta.value_type.name(),
                        "modified": is_modified,
                        "description": meta.description
                    }),
                );
            }

            if !section_items.is_empty() {
                result.insert(
                    section.to_string(),
                    serde_json::Value::Object(section_items),
                );
            }
        }

        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::Value::Object(result))?
        );
    } else {
        let theme = ActiveTheme::default();
        let mut out = std::io::stdout();
        let mut fmt = Formatter::stdout(&mut out, theme);

        fmt.subheading("Configuration:")?;
        fmt.newline()?;

        for section in reg.sections() {
            if let Some(filter_section) = &args.section {
                if !section.starts_with(filter_section.as_str()) {
                    continue;
                }
            }

            let section_configs: Vec<_> = reg
                .configs_in_section(section)
                .into_iter()
                .filter(|m| args.all || !m.advanced)
                .filter(|m| {
                    if args.modified {
                        let value = items
                            .get(m.key)
                            .cloned()
                            .unwrap_or_else(|| m.default.to_string());
                        m.is_modified(&value)
                    } else {
                        true
                    }
                })
                .collect();

            if section_configs.is_empty() {
                continue;
            }

            // Section header
            let section_desc = reg.section_description(section).unwrap_or(section);
            fmt.write_accent(&format!("## {section}"))?;
            fmt.write_raw(" ")?;
            fmt.write_muted(&format!("({section_desc})"))?;
            fmt.newline()?;

            for meta in section_configs {
                let value = items
                    .get(meta.key)
                    .cloned()
                    .unwrap_or_else(|| meta.default.to_string());
                let is_modified = meta.is_modified(&value);

                let modified_marker = if is_modified { "*" } else { " " };
                let advanced_marker = if meta.advanced { " [adv]" } else { "" };

                fmt.write_raw(&format!("  {modified_marker}"))?;
                if is_modified {
                    let warn_color = fmt.theme().palette.status_warning;
                    fmt.write_colored(meta.key, warn_color)?;
                    fmt.write_raw(" = ")?;
                    let success_color = fmt.theme().palette.status_success;
                    fmt.write_bold_colored(&value, success_color)?;
                } else {
                    fmt.write_raw(meta.key)?;
                    fmt.write_raw(" = ")?;
                    fmt.write_muted(&value)?;
                }
                if !advanced_marker.is_empty() {
                    fmt.write_muted(advanced_marker)?;
                }
                fmt.newline()?;
            }
            fmt.newline()?;
        }

        if !args.all {
            let advanced_count = reg.advanced_configs().len();
            fmt.write_muted(&format!(
                "(Use --all to show {advanced_count} advanced options)"
            ))?;
            fmt.newline()?;
        }

        if Config::is_sync_disabled() {
            fmt.newline()?;
            fmt.warning("Sync is disabled via MEM_SYNC_DISABLED environment variable")?;
        }
    }

    Ok(())
}

pub(crate) fn execute_describe(
    args: &ConfigDescribeArgs,
    cli: &Cli,
    cas_root: &Path,
) -> anyhow::Result<()> {
    let config = Config::load(cas_root)?;
    let reg = registry();

    if let Some(meta) = reg.get(&args.key) {
        let current_value = config
            .get(&args.key)
            .unwrap_or_else(|| meta.default.to_string());
        let is_modified = meta.is_modified(&current_value);

        if cli.json {
            let json = serde_json::json!({
                "key": meta.key,
                "name": meta.name,
                "section": meta.section,
                "description": meta.description,
                "type": meta.value_type.name(),
                "default": meta.default,
                "current_value": current_value,
                "modified": is_modified,
                "advanced": meta.advanced,
                "requires_feature": meta.requires_feature,
                "constraint": format_constraint(&meta.constraint),
                "examples": meta.value_type.examples()
            });
            println!("{}", serde_json::to_string_pretty(&json)?);
        } else {
            let theme = ActiveTheme::default();
            let mut out = std::io::stdout();
            let mut fmt = Formatter::stdout(&mut out, theme);

            fmt.write_bold(&format!("## {}", meta.name))?;
            fmt.newline()?;
            fmt.newline()?;
            fmt.field("Key", meta.key)?;
            fmt.field("Section", meta.section)?;
            fmt.field("Type", meta.value_type.name())?;
            fmt.newline()?;
            fmt.subheading("Description:")?;
            fmt.write_raw(&format!("  {}", meta.description))?;
            fmt.newline()?;
            fmt.newline()?;

            let current_display = if is_modified {
                format!("{current_value} (modified)")
            } else {
                current_value.clone()
            };
            fmt.field("Current", &current_display)?;
            fmt.write_raw("Default:     ")?;
            fmt.write_muted(meta.default)?;
            fmt.newline()?;

            let constraint_str = format_constraint(&meta.constraint);
            if !constraint_str.is_empty() {
                fmt.field("Constraint", &constraint_str)?;
            }

            fmt.newline()?;
            fmt.write_raw("Examples:    ")?;
            fmt.write_muted(&meta.value_type.examples().join(", "))?;
            fmt.newline()?;

            if meta.advanced {
                fmt.newline()?;
                fmt.write_muted("Note: This is an advanced option")?;
                fmt.newline()?;
            }

            if let Some(feature) = meta.requires_feature {
                fmt.write_muted(&format!("Note: Requires feature '{feature}'"))?;
                fmt.newline()?;
            }
        }
        Ok(())
    } else {
        if cli.json {
            println!(r#"{{"error":"unknown key","key":"{}"}}"#, args.key);
        } else {
            eprintln!("Unknown config key: {}", args.key);

            // Suggest similar keys
            let suggestions = reg.search(&args.key);
            if !suggestions.is_empty() {
                eprintln!("\nDid you mean:");
                for meta in suggestions.iter().take(5) {
                    eprintln!("  {} - {}", meta.key, meta.name);
                }
            }
        }
        std::process::exit(1);
    }
}

pub(crate) fn execute_diff(
    args: &ConfigDiffArgs,
    cli: &Cli,
    cas_root: &Path,
) -> anyhow::Result<()> {
    let config = Config::load(cas_root)?;
    let reg = registry();

    let items: HashMap<String, String> = config.list().into_iter().collect();

    let mut diffs: Vec<(&str, String, &str)> = Vec::new();

    for key in reg.all_keys() {
        if let Some(meta) = reg.get(key) {
            if !args.all && meta.advanced {
                continue;
            }

            let current = items
                .get(key)
                .cloned()
                .unwrap_or_else(|| meta.default.to_string());
            if meta.is_modified(&current) {
                diffs.push((key, current, meta.default));
            }
        }
    }

    if cli.json {
        let json: Vec<_> = diffs
            .iter()
            .map(|(key, current, default)| {
                serde_json::json!({
                    "key": key,
                    "current": current,
                    "default": default
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else {
        let theme = ActiveTheme::default();
        let mut out = std::io::stdout();
        let mut fmt = Formatter::stdout(&mut out, theme);

        if diffs.is_empty() {
            fmt.success("No differences from default configuration")?;
        } else {
            fmt.write_accent("Configuration differences from defaults:")?;
            fmt.newline()?;
            fmt.newline()?;
            for (key, current, default) in &diffs {
                fmt.write_raw("  ")?;
                let warn_color = fmt.theme().palette.status_warning;
                fmt.write_colored(key, warn_color)?;
                fmt.write_raw(" = ")?;
                let success_color = fmt.theme().palette.status_success;
                fmt.write_bold_colored(current, success_color)?;
                fmt.newline()?;
                fmt.write_raw("    ")?;
                fmt.write_muted(&format!("default: {default}"))?;
                fmt.newline()?;
            }
            fmt.newline()?;
            fmt.write_raw(&format!("{} option(s) modified", diffs.len()))?;
            fmt.newline()?;
        }
    }

    Ok(())
}

pub(crate) fn execute_reset(
    args: &ConfigResetArgs,
    cli: &Cli,
    cas_root: &Path,
) -> anyhow::Result<()> {
    let mut config = Config::load(cas_root)?;
    let reg = registry();

    if args.key == "all" {
        if !args.force {
            eprintln!("This will reset ALL config options to defaults.");
            eprintln!("Use --force to confirm.");
            std::process::exit(1);
        }

        // Reset to default config
        config = Config::default();
        config.save(cas_root)?;

        if cli.json {
            println!(r#"{{"reset":"all","count":{}}}"#, reg.count());
        } else {
            println!("Reset all {} options to defaults", reg.count());
        }
    } else if let Some(meta) = reg.get(&args.key) {
        let old_value = config.get(&args.key).unwrap_or_default();
        config.set(&args.key, meta.default)?;
        config.save(cas_root)?;

        if cli.json {
            let json = serde_json::json!({
                "key": args.key,
                "old_value": old_value,
                "new_value": meta.default
            });
            println!("{}", serde_json::to_string(&json)?);
        } else {
            println!("Reset {} to default: {}", args.key, meta.default);
            if old_value != meta.default {
                println!("  (was: {old_value})");
            }
        }
    } else {
        if cli.json {
            println!(r#"{{"error":"unknown key","key":"{}"}}"#, args.key);
        } else {
            eprintln!("Unknown config key: {}", args.key);
        }
        std::process::exit(1);
    }

    Ok(())
}

pub(crate) fn execute_search(
    args: &ConfigSearchArgs,
    cli: &Cli,
    cas_root: &Path,
) -> anyhow::Result<()> {
    let config = Config::load(cas_root)?;
    let reg = registry();

    let results = reg.search(&args.query);

    if cli.json {
        let json: Vec<_> = results
            .iter()
            .map(|meta| {
                let current = config
                    .get(meta.key)
                    .unwrap_or_else(|| meta.default.to_string());
                serde_json::json!({
                    "key": meta.key,
                    "name": meta.name,
                    "description": meta.description,
                    "current_value": current,
                    "default": meta.default,
                    "type": meta.value_type.name()
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else if results.is_empty() {
        println!("No config options matching '{}'", args.query);
    } else {
        println!("Config options matching '{}':", args.query);
        println!();
        for meta in results {
            let current = config
                .get(meta.key)
                .unwrap_or_else(|| meta.default.to_string());
            let modified = if meta.is_modified(&current) { "*" } else { " " };
            println!("{}{}", modified, meta.key);
            println!("    {} = {}", meta.name, current);
            println!("    {}", truncate_description(meta.description, 60));
            println!();
        }
    }

    Ok(())
}
