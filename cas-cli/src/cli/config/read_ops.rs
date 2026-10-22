use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::cli::Cli;
use crate::cli::config::{
    ConfigDescribeArgs, ConfigDiffArgs, ConfigGetArgs, ConfigListArgs, ConfigResetArgs,
    ConfigSearchArgs, ConfigSetArgs,
};
use crate::config::{Config, registry};
use crate::ui::components::Formatter;
use crate::ui::theme::ActiveTheme;

use crate::cli::config::operator_ops::guard_operator_config;
use crate::cli::config::util::{format_constraint, truncate_description};
use crate::config::operator_policy::InvocationContext;

pub(super) fn host_scoped_key(key: &str) -> bool {
    key == "viktor.enabled" || key.starts_with("hub.")
}

fn config_root_for_key(key: &str, cas_root: &Path) -> PathBuf {
    if host_scoped_key(key) {
        crate::config::host_config_dir()
    } else {
        cas_root.to_path_buf()
    }
}

// Plain `get` stays a machine-readable scalar for `gh --repo "$(cas config get
// issues.repo)"`. Structured and human multi-field views expose provenance.
fn origin_note(config: &Config, key: &str, value: &str) -> &'static str {
    if key == "issues.repo" && !value.is_empty() && config.issue_repo_registry().project.is_none() {
        " (from origin; unset in config)"
    } else {
        ""
    }
}

fn blank_component_note(config: &Config, key: &str) -> Option<&'static str> {
    if key != "issues.components.cassy" { return None; }
    let raw = config.issues.as_ref()?.components.as_ref()?.cassy.as_deref()?;
    if !raw.trim().is_empty() { return None; }
    Some(if config.cassy_issue_legacy_repo().is_some() {
        "(blank: legacy issues.repo fallback)"
    } else {
        "(blank: issues.repo unset; using component default)"
    })
}

fn with_issue_repo_source(
    mut json: serde_json::Value,
    config: &Config,
    key: &str,
    value: &str,
) -> serde_json::Value {
    if key == "issues.repo" {
        json["source"] = serde_json::json!(if !origin_note(config, key, value).is_empty() {
            "origin"
        } else if config.issue_repo_registry().project.is_some() {
            "config"
        } else {
            "unset"
        });
        json["configured"] = serde_json::json!(config.issue_repo_registry().project.is_some());
    }
    if blank_component_note(config, key).is_some() {
        let legacy = config.cassy_issue_legacy_repo();
        json["raw_blank"] = serde_json::json!(true);
        json["report_repo"] = serde_json::json!(legacy.map(str::to_owned)
            .unwrap_or_else(|| config.issue_repo_registry().cassy));
        json["report_source"] = serde_json::json!(if legacy.is_some() {
            "issues.repo"
        } else {
            "issues.components.cassy"
        });
    }
    json
}

fn get_output(
    config: &Config, key: &str, value: &str, json: bool,
) -> anyhow::Result<(String, Option<&'static str>)> {
    if json {
        let meta = registry().get(key);
        let json = serde_json::json!({
            "key": key, "value": value,
            "type": meta.map(|m| m.value_type.name()),
            "default": meta.map(|m| m.default),
            "modified": meta.map(|m| m.is_modified(value)).unwrap_or(false)
        });
        Ok((serde_json::to_string_pretty(&with_issue_repo_source(json, config, key, value))?, None))
    } else {
        Ok((value.to_owned(), blank_component_note(config, key)))
    }
}

pub(crate) fn execute_get(args: &ConfigGetArgs, cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    let config_root = config_root_for_key(&args.key, cas_root);
    let config = Config::load(&config_root)?;
    execute_get_internal(&config, &args.key, cli, &config_root)
}

fn execute_get_internal(
    config: &Config,
    key: &str,
    cli: &Cli,
    cas_root: &Path,
) -> anyhow::Result<()> {
    if let Some(value) = config.get_resolved(key, cas_root) {
        let (output, note) = get_output(config, key, &value, cli.json)?;
        println!("{output}");
        if let Some(note) = note {
            eprintln!("{note}");
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
    let config_root = config_root_for_key(&args.key, cas_root);
    let mut config = Config::load(&config_root)?;
    execute_set_internal(&mut config, &args.key, &args.value, &config_root, cli)
}

/// Set and save one key; a security-relevant key needs the operator
/// (cas-0d4f0). Returns the previous value.
pub(crate) fn set_config_value(
    config: &mut Config,
    key: &str,
    value: &str,
    cas_root: &Path,
    context: impl FnOnce() -> InvocationContext,
) -> anyhow::Result<String> {
    update_config_in_place(
        config,
        key,
        cas_root,
        |before, after| {
            let old_value = before.get(key).unwrap_or_default();
            after.set(key, value)?;
            Ok(old_value)
        },
        |before, after| guard_operator_config(before, after, context),
    )
}

/// Apply a validated mutation through the config editor without serializing
/// unrelated defaults back into the document. The guard lets the operator-only
/// write-roots path retain its separate cgroup policy.
pub(crate) fn update_config_in_place<T>(
    config: &mut Config,
    key: &str,
    cas_root: &Path,
    update: impl FnOnce(&Config, &mut Config) -> anyhow::Result<T>,
    guard: impl FnOnce(&Config, &Config) -> anyhow::Result<()>,
) -> anyhow::Result<T> {
    let _lock = crate::config::lock_project_config(cas_root)?;
    let path = cas_root.join("config.toml");
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    let before: Config = if raw.is_empty() {
        config.clone()
    } else {
        toml::from_str(&raw)?
    };
    let mut after = before.clone();
    let result = update(&before, &mut after)?;

    let serialized_after = toml::to_string(&after)?;
    let after_document = toml_edit::Document::parse(serialized_after)?;
    let mut document = toml_edit::Document::parse(raw.as_str())?.into_mut();
    let (serialized_path, stored_path, alternate_path) =
        config_document_paths(&document, key);
    let replacement = config_document_item(after_document.as_item(), serialized_path).cloned();
    set_config_document_item(&mut document, stored_path, replacement)?;
    if let Some(alternate_path) = alternate_path {
        set_config_document_item(&mut document, alternate_path, None)?;
    }
    if let Some(marker) = upkeep_observe_marker_for_set(key) {
        set_config_document_item(&mut document, marker, None)?;
    }
    let contents = document.to_string();
    let saved: Config = toml::from_str(&contents)?;
    guard(&before, &saved)?;
    if contents != raw {
        crate::config::atomic_replace_project_config(&path, &contents)?;
    }
    *config = saved;
    if key == "viktor.enabled" {
        crate::cli::viktor::reconcile_managed_proxy(config.viktor_enabled())?;
    }
    Ok(result)
}

fn config_document_item<'a>(item: &'a toml_edit::Item, path: &str) -> Option<&'a toml_edit::Item> {
    path.split('.')
        .try_fold(item, |item, part| item.as_table_like()?.get(part))
}

fn config_document_paths<'a>(
    document: &toml_edit::DocumentMut,
    key: &'a str,
) -> (&'a str, &'a str, Option<&'a str>) {
    let (serialized_path, alternate_path) = match key {
        "factory.worker_build_jobs" | "factory.cargo_build_jobs" => {
            ("factory.cargo_build_jobs", Some("factory.worker_build_jobs"))
        }
        "staging.staging_dir" | "staging.large_artifact_dir" => {
            ("staging.staging_dir", Some("staging.large_artifact_dir"))
        }
        _ => (key, None),
    };
    let stored_path = if config_document_item(document.as_item(), serialized_path).is_some() {
        serialized_path
    } else if let Some(alternate_path) = alternate_path.filter(|path| {
        config_document_item(document.as_item(), path).is_some()
    }) {
        alternate_path
    } else {
        serialized_path
    };
    let remove_path = alternate_path.map(|alternate_path| {
        if stored_path == serialized_path {
            alternate_path
        } else {
            serialized_path
        }
    });
    (serialized_path, stored_path, remove_path)
}

fn upkeep_observe_marker_for_set(key: &str) -> Option<&'static str> {
    match key {
        "upkeep.codemap" => Some("upkeep.observe_since.codemap"),
        "upkeep.product_overview" => Some("upkeep.observe_since.product_overview"),
        "qa.registry.enforce" => Some("upkeep.observe_since.registry"),
        _ => None,
    }
}

fn set_config_document_item(
    document: &mut toml_edit::DocumentMut,
    path: &str,
    replacement: Option<toml_edit::Item>,
) -> anyhow::Result<()> {
    let parts: Vec<_> = path.split('.').collect();
    anyhow::ensure!(!parts.is_empty(), "config key must not be empty");
    set_config_document_path(document.as_item_mut(), &parts, replacement)
}

fn set_config_document_path(
    item: &mut toml_edit::Item,
    parts: &[&str],
    replacement: Option<toml_edit::Item>,
) -> anyhow::Result<()> {
    let Some((part, rest)) = parts.split_first() else {
        return Ok(());
    };
    let parent_is_inline = item.as_value().is_some_and(toml_edit::Value::is_inline_table);
    let table = item
        .as_table_like_mut()
        .ok_or_else(|| anyhow::anyhow!("config key path parent for {part} is not a table"))?;

    if rest.is_empty() {
        if let Some(replacement) = replacement {
            let mut replacement = replacement;
            if let Some(existing) = table.get_mut(part) {
                let old_decor = existing.as_value().map(|value| value.decor().clone());
                if let (Some(decor), Some(new_value)) = (old_decor, replacement.as_value_mut()) {
                    *new_value.decor_mut() = decor;
                }
                *existing = replacement;
            } else {
                table.insert(part, replacement);
            }
        } else {
            table.remove(part);
        }
        return Ok(());
    }

    if table.get(part).is_none() {
        if replacement.is_none() {
            return Ok(());
        }
        let child = if parent_is_inline {
            toml_edit::Item::Value(toml_edit::Value::InlineTable(
                toml_edit::InlineTable::new(),
            ))
        } else {
            let mut child = toml_edit::Table::new();
            child.set_implicit(true);
            toml_edit::Item::Table(child)
        };
        table.insert(part, child);
    }
    let child = table
        .get_mut(part)
        .expect("just-created or previously present config table");
    set_config_document_path(child, rest, replacement)
}

/// Remove the stored override without serializing defaults back into the file.
/// Keep the same operator guard and atomic writer as other config mutations.
fn reset_config_value(
    key: &str,
    cas_root: &Path,
) -> anyhow::Result<(String, String)> {
    anyhow::ensure!(key != "factory.write_roots",
        "factory.write_roots is operator-only; use `cas config set factory.write_roots \"\"` to clear it");
    let _lock = crate::config::lock_project_config(cas_root)?;
    let path = cas_root.join("config.toml");
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    let before: Config = toml::from_str(&raw)?;
    let mut document = toml_edit::Document::parse(&raw)?.into_mut();
    let mut parts: Vec<_> = key.split('.').collect();
    let leaf = parts.pop().expect("a config key has at least one part");
    let parent = parts.into_iter().try_fold(document.as_item_mut(), |item, part| {
        item.as_table_like_mut()?.get_mut(part)
    });
    let removed = parent.and_then(|item| item.as_table_like_mut()?.remove(leaf)).is_some();
    let contents = document.to_string();
    let after: Config = toml::from_str(&contents)?;
    guard_operator_config(&before, &after, InvocationContext::from_process)?;
    if removed {
        crate::config::atomic_replace_project_config(&path, &contents)?;
    }
    if key == "viktor.enabled" {
        crate::cli::viktor::reconcile_managed_proxy(after.viktor_enabled())?;
    }
    Ok((before.get(key).unwrap_or_default(), after.get(key).unwrap_or_default()))
}

#[derive(serde::Serialize)]
struct EnforcementWarning {
    artifact: &'static str,
    drift_count: usize,
    unit: &'static str,
    measured_ref: String,
}

fn enforcement_warning(
    config: &Config,
    key: &str,
    old: &str,
    value: &str,
    cas_root: &Path,
) -> Option<EnforcementWarning> {
    if value != "enforce"
        || !matches!(old, "off" | "observe")
        || upkeep_observe_marker_for_set(key).is_none()
    {
        return None;
    }
    use crate::hooks::handlers::handlers_events::docs_freshness::{Measurement, UpkeepReport};
    let report = UpkeepReport::at(&Measurement::integration_for_store(cas_root), &config.qa());
    let artifact = report.for_key(key)?;
    let count = artifact.drift_count.filter(|count| *count > 0)?;
    Some(EnforcementWarning {
        artifact: artifact.name,
        drift_count: count,
        unit: artifact.unit,
        measured_ref: report.measured_ref,
    })
}

fn execute_set_internal(
    config: &mut Config,
    key: &str,
    value: &str,
    cas_root: &std::path::Path,
    cli: &Cli,
) -> anyhow::Result<()> {
    let old_value = set_config_value(
        config,
        key,
        value,
        cas_root,
        InvocationContext::from_process,
    )?;

    // Measurement is best effort after the successful save, outside its config lock.
    let warnings: Vec<_> = enforcement_warning(config, key, &old_value, value, cas_root)
        .into_iter()
        .collect();
    if cli.json {
        let json = serde_json::json!({
            "key": key,
            "old_value": old_value,
            "new_value": value,
            "warnings": warnings
        });
        println!("{}", serde_json::to_string(&json)?);
    } else {
        let theme = ActiveTheme::default();
        let mut out = std::io::stdout();
        let mut fmt = Formatter::stdout(&mut out, theme);
        let shadowed_mode = (key == "hub.tailscale_serve")
            .then(|| config.hub.as_ref().and_then(|hub| hub.transport_mode))
            .flatten();
        if let Some(mode) = shadowed_mode {
            let effective_value = config.get(key).unwrap_or_else(|| value.to_owned());
            let requested_mode = if value == "true" { "auto" } else { "loopback" };
            fmt.warning(&format!(
                "Legacy {key} = {value} saved; ignored by hub.transport_mode."
            ))?;
            fmt.write_raw(&format!(
                "  Effective value: {key} = {effective_value} (transport_mode = {}).",
                mode.as_str()
            ))?;
            fmt.newline()?;
            fmt.write_raw(&format!(
                "  Use: cas config set hub.transport_mode {requested_mode}"
            ))?;
            fmt.newline()?;
        } else {
            fmt.success(&format!("Set {key} = {value}"))?;
            if !old_value.is_empty() && old_value != value {
                fmt.write_raw("  ")?;
                fmt.write_muted(&format!("was: {old_value}"))?;
                fmt.newline()?;
            }
        }
        for warning in &warnings {
            fmt.write_raw(&format!(
                "WARNING: {} drift: {} {}",
                warning.artifact, warning.drift_count, warning.unit
            ))?;
            fmt.newline()?;
            fmt.write_raw(&crate::cli::qa::measured_ref_line(&warning.measured_ref))?;
            fmt.newline()?;
            fmt.write_raw("Review: cas qa journeys --status --json")?;
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
    let mut items: HashMap<String, String> = config.list().into_iter().collect();
    let host_config = Config::load(&crate::config::host_config_dir())?;
    items.insert("viktor.enabled".into(), host_config.viktor_enabled().to_string());
    for (key, value) in host_config.list() {
        if host_scoped_key(&key) {
            items.insert(key, value);
        }
    }
    items.insert(
        "issues.repo".into(),
        config
            .get_resolved("issues.repo", cas_root)
            .unwrap_or_default(),
    );

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
                    with_issue_repo_source(
                        serde_json::json!({
                            "value": value,
                            "default": meta.default,
                            "type": meta.value_type.name(),
                            "modified": is_modified,
                            "description": meta.description
                        }),
                        &config,
                        meta.key,
                        &value,
                    ),
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
                    let warn_color = fmt.theme().palette.text_warning;
                    fmt.write_colored(meta.key, warn_color)?;
                    fmt.write_raw(" = ")?;
                    let success_color = fmt.theme().palette.text_success;
                    fmt.write_bold_colored(&value, success_color)?;
                } else {
                    fmt.write_raw(meta.key)?;
                    fmt.write_raw(" = ")?;
                    fmt.write_muted(&value)?;
                }
                fmt.write_muted(origin_note(&config, meta.key, &value))?;
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
            .get_resolved(&args.key, cas_root)
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
                "examples": meta.examples()
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&with_issue_repo_source(
                    json,
                    &config,
                    &args.key,
                    &current_value
                ))?
            );
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
            fmt.field(
                "Current",
                &format!(
                    "{current_display}{}",
                    origin_note(&config, &args.key, &current_value)
                ),
            )?;
            fmt.write_raw("Default:     ")?;
            fmt.write_muted(meta.default)?;
            fmt.newline()?;

            let constraint_str = format_constraint(&meta.constraint);
            if !constraint_str.is_empty() {
                fmt.field("Constraint", &constraint_str)?;
            }

            fmt.newline()?;
            fmt.write_raw("Examples:    ")?;
            fmt.write_muted(&meta.examples().join(", "))?;
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
                let warn_color = fmt.theme().palette.text_warning;
                fmt.write_colored(key, warn_color)?;
                fmt.write_raw(" = ")?;
                let success_color = fmt.theme().palette.text_success;
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
    let config_root = config_root_for_key(&args.key, cas_root);
    let mut config = Config::load(&config_root)?;
    let reg = registry();

    if args.key == "all" {
        if !args.force {
            eprintln!("This will reset ALL config options to defaults.");
            eprintln!("Use --force to confirm.");
            std::process::exit(1);
        }

        // Reset to default config
        let reset = Config::default();
        guard_operator_config(&config, &reset, InvocationContext::from_process)?;
        config = reset;
        config.save(&config_root)?;

        if cli.json {
            println!(r#"{{"reset":"all","count":{}}}"#, reg.count());
        } else {
            println!("Reset all {} options to defaults", reg.count());
        }
    } else if reg.get(&args.key).is_some() {
        let (old_value, new_value) = reset_config_value(&args.key, &config_root)?;

        if cli.json {
            let json = serde_json::json!({
                "key": args.key,
                "old_value": old_value,
                "new_value": new_value
            });
            println!("{}", serde_json::to_string(&json)?);
        } else {
            println!("Reset {} to default: {}", args.key, new_value);
            if old_value != new_value {
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
                    .get_resolved(meta.key, cas_root)
                    .unwrap_or_else(|| meta.default.to_string());
                with_issue_repo_source(
                    serde_json::json!({
                        "key": meta.key,
                        "name": meta.name,
                        "description": meta.description,
                        "current_value": current,
                        "default": meta.default,
                        "type": meta.value_type.name()
                    }),
                    &config,
                    meta.key,
                    &current,
                )
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
                .get_resolved(meta.key, cas_root)
                .unwrap_or_else(|| meta.default.to_string());
            let modified = if meta.is_modified(&current) { "*" } else { " " };
            println!("{}{}", modified, meta.key);
            println!(
                "    {} = {}{}",
                meta.name,
                current,
                origin_note(&config, meta.key, &current)
            );
            println!("    {}", truncate_description(meta.description, 60));
            println!();
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_reset_maps_registry_keys_to_serialized_toml_paths() {
        const SET_REJECTED_REGISTRY_KEYS: &[&str] = &[
            "hooks.stop.duplicate_detection_enabled",
            "hooks.stop.duplicate_detection_threshold",
            "hooks.stop.learning_review_enabled",
            "hooks.stop.learning_review_threshold",
            "hooks.stop.rule_review_enabled",
            "hooks.stop.rule_review_threshold",
            "notifications.on_auth_success",
            "notifications.on_idle_prompt",
            "notifications.on_permission_prompt",
            "notifications.webhook_url",
        ];

        let registry = crate::config::registry();
        let mut keys = registry.all_keys();
        keys.extend(["factory.cargo_build_jobs", "staging.large_artifact_dir"]);
        keys.sort_unstable();
        keys.dedup();

        for key in keys {
            let document = if key == "factory.write_roots" {
                let mut config = Config::default();
                config.set_factory_write_roots(vec!["/tmp/cas-config-path".into()]);
                toml_edit::Document::parse(toml::to_string(&config).unwrap())
                    .unwrap()
                    .into_mut()
            } else if SET_REJECTED_REGISTRY_KEYS.contains(&key) {
                let (parent, leaf) = key.rsplit_once('.').expect("nested config key");
                toml_edit::Document::parse(format!("[{parent}]\n{leaf} = \"sentinel\"\n"))
                    .unwrap()
                    .into_mut()
            } else {
                let meta = registry.get(key).or_else(|| match key {
                    "factory.cargo_build_jobs" => registry.get("factory.worker_build_jobs"),
                    "staging.large_artifact_dir" => registry.get("staging.staging_dir"),
                    _ => None,
                }).expect("registered key or compatibility alias");
                let mut candidates = vec![meta.default.to_owned()];
                if key == "factory.scratch_root" {
                    candidates.push("/tmp/cas-config-path".into());
                } else if key == "qa.deployed_origins" {
                    candidates.push("https://example.com".into());
                } else if key == "slack.stakeholder_contacts" {
                    candidates.push("[]".into());
                } else if key.starts_with("issues.") {
                    candidates.push("example/repo".into());
                }
                if let crate::config::Constraint::OneOf(values) = &meta.constraint {
                    candidates.extend(values.iter().cloned());
                }
                candidates.extend(meta.examples().into_iter().map(str::to_owned));
                candidates.extend(
                    ["example", "example/repo", "/tmp/cas-config-path", "https://example.com"]
                        .into_iter()
                        .map(str::to_owned),
                );
                candidates.sort();
                candidates.dedup();

                let mut resolved = None;
                for value in candidates {
                    let mut config = Config::default();
                    if config.set(key, &value).is_err() {
                        continue;
                    }
                    let raw = toml::to_string(&config).unwrap();
                    let candidate = toml_edit::Document::parse(raw).unwrap().into_mut();
                    let (_, stored_path, _) = config_document_paths(&candidate, key);
                    if config_document_item(candidate.as_item(), stored_path).is_some() {
                        resolved = Some(candidate);
                        break;
                    }
                }
                resolved.unwrap_or_else(|| panic!("no TOML value materialized registry key {key}"))
            };

            let (_, stored_path, _) = config_document_paths(&document, key);
            assert!(
                config_document_item(document.as_item(), stored_path).is_some(),
                "registry key {key} did not resolve to a serialized TOML path {stored_path} in:\n{raw}"
            );
        }
    }

    #[test]
    fn config_reset_removes_canonical_toml_path_for_registry_alias() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(dir.path()).unwrap();
        std::fs::write(
            root.join("config.toml"),
            "[factory]\ncargo_build_jobs = \"4\"\n",
        )
        .unwrap();

        reset_config_value("factory.worker_build_jobs", &root).unwrap();

        let saved = std::fs::read_to_string(root.join("config.toml")).unwrap();
        let document: toml::Value = toml::from_str(&saved).unwrap();
        assert!(document["factory"].get("cargo_build_jobs").is_none(), "{saved}");
    }

    #[test]
    fn cas_337b_config_reset_removes_key_and_restores_fallback() {
        use clap::Parser;
        let env = crate::test_support::TestEnvGuard::temp_home();
        let root = env.home().join(".cas");
        std::fs::create_dir_all(&root).unwrap();
        let original = "# keep host policy\n[hub]\ntailscale_serve = false\n";
        std::fs::write(root.join("config.toml"), format!("{original}transport_mode = \"required\"\n")).unwrap();
        let cli = Cli::parse_from(["cas", "config", "reset", "hub.transport_mode"]);
        execute_reset(&ConfigResetArgs {
            key: "hub.transport_mode".into(),
            force: false,
            store: Default::default(),
        }, &cli, &root).unwrap();
        let saved = std::fs::read_to_string(root.join("config.toml")).unwrap();
        let document: toml::Value = toml::from_str(&saved).unwrap();
        assert!(document["hub"].get("transport_mode").is_none(), "reset persisted an override: {saved}");
        assert_eq!(Config::load(&root).unwrap().hub.unwrap().transport_mode(), crate::config::HubTransportMode::Loopback);
        assert_eq!(saved, original, "reset must preserve unrelated settings and comments");
    }

    #[test]
    fn config_get_explains_raw_blank_cassy_filing_fallback_cas_02a3() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::tempdir().unwrap();
        let key = "issues.components.cassy";
        let default = crate::config::DEFAULT_CASSY_ISSUES_REPO;
        for (component, project, note, report_repo, report_source) in [
            (Some(""), Some("example/project"), Some("legacy issues.repo fallback"), "example/project", "issues.repo"),
            (Some("  "), None, Some("issues.repo unset"), default, key),
            (None, Some("example/project"), None, default, key),
            (Some("example/component"), Some("example/project"), None, "example/component", key),
        ] {
            // Raw configuration is the documented legacy opt-out; config set
            // with blank intentionally clears the override instead.
            let mut config = Config::default();
            let issues = config.issues.get_or_insert_with(Default::default);
            issues.repo = project.map(str::to_string);
            issues.components.get_or_insert_with(Default::default).cassy = component.map(str::to_string);
            config.save(dir.path()).unwrap();
            let before = std::fs::read(dir.path().join("config.toml")).unwrap();
            let config = Config::load(dir.path()).unwrap();
            let value = config.get_resolved(key, dir.path()).unwrap();
            let (scalar, explanation) = get_output(&config, key, &value, false).unwrap();
            // pin: config get scalars are consumed by shell substitutions.
            assert_eq!(scalar, component.filter(|value| !value.trim().is_empty()).unwrap_or(default));
            match note {
                Some(expected) => assert!(explanation.is_some_and(|text| text.contains(expected)), "blank opt-out lost: {explanation:?}"),
                None => assert_eq!(explanation, None),
            }
            let (document, stderr) = get_output(&config, key, &value, true).unwrap();
            let document: serde_json::Value = serde_json::from_str(&document).unwrap();
            assert!(stderr.is_none());
            assert_eq!(document["value"], scalar);
            if note.is_some() {
                assert_eq!(document["raw_blank"], true);
                assert_eq!(document["report_repo"], report_repo);
                assert_eq!(document["report_source"], report_source);
            } else {
                assert!(document.get("raw_blank").is_none());
            }
            assert_eq!(std::fs::read(dir.path().join("config.toml")).unwrap(), before);
        }
    }
}
