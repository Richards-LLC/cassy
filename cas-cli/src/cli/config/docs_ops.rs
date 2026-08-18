use std::io::{self, Write};

use crate::cli::Cli;
use crate::cli::config::{CompletionShell, ConfigCompletionsArgs, ConfigDocsArgs};
use crate::config::registry;
use crate::ui::components::{Formatter, Header, Renderable, StatusLine};
use crate::ui::theme::ActiveTheme;

use crate::cli::config::util::format_constraint;

pub(crate) fn execute_docs(args: &ConfigDocsArgs, cli: &Cli) -> anyhow::Result<()> {
    let reg = registry();

    match args.format.as_str() {
        "markdown" | "md" => {
            if args.section.is_some() {
                // Section-filtered: use simple table format
                print_help_markdown(&args.section, reg)?;
            } else {
                // Full docs: use agent-friendly format with frontmatter
                let mut stdout = io::stdout();
                writeln!(stdout, "{}", reg.generate_markdown())?;
            }
        }
        "man" => print_help_man(&args.section, reg)?,
        _ => print_help_text(&args.section, reg, cli.json)?,
    }

    Ok(())
}

fn print_help_text(
    section_filter: &Option<String>,
    reg: &crate::config::ConfigRegistry,
    json: bool,
) -> io::Result<()> {
    if json {
        let mut sections_json = serde_json::Map::new();

        for section in reg.sections() {
            if let Some(filter) = section_filter {
                if !section.starts_with(filter.as_str()) {
                    continue;
                }
            }

            let configs: Vec<_> = reg
                .configs_in_section(section)
                .iter()
                .map(|meta| {
                    serde_json::json!({
                        "key": meta.key,
                        "name": meta.name,
                        "description": meta.description,
                        "type": meta.value_type.name(),
                        "default": meta.default,
                        "advanced": meta.advanced,
                        "constraint": format_constraint(&meta.constraint)
                    })
                })
                .collect();

            sections_json.insert(
                section.to_string(),
                serde_json::json!({
                    "description": reg.section_description(section),
                    "options": configs
                }),
            );
        }

        match serde_json::to_string_pretty(&serde_json::Value::Object(sections_json)) {
            Ok(json_text) => println!("{json_text}"),
            Err(err) => {
                let mut stderr = io::stderr();
                let theme = ActiveTheme::default();
                let mut fmt = Formatter::stdout(&mut stderr, theme);
                let _ = StatusLine::error(format!("Failed to serialize config docs: {err}"))
                    .render(&mut fmt);
            }
        }
        return Ok(());
    }

    let theme = ActiveTheme::default();
    let mut stdout = io::stdout();
    let mut fmt = Formatter::stdout(&mut stdout, theme);

    Header::h1("Cassy Configuration Reference").render(&mut fmt)?;
    fmt.newline()?;
    fmt.info("Use 'cas config <key> <value>' to set options.")?;
    fmt.info("Use 'cas config describe <key>' for detailed info.")?;
    fmt.newline()?;

    for section in reg.sections() {
        if let Some(filter) = section_filter {
            if !section.starts_with(filter.as_str()) {
                continue;
            }
        }

        let section_desc = reg.section_description(section).unwrap_or("");
        fmt.subheading(&format!("{} - {}", section.to_uppercase(), section_desc))?;
        fmt.newline()?;

        for meta in reg.configs_in_section(section) {
            let advanced = if meta.advanced { " [advanced]" } else { "" };
            fmt.bullet(&format!("{}{}", meta.key, advanced))?;
            fmt.write_muted(&format!("    {}", meta.name))?;
            fmt.newline()?;
            fmt.write_muted(&format!(
                "    Type: {} | Default: {}",
                meta.value_type.name(),
                meta.default
            ))?;
            fmt.newline()?;
            fmt.write_muted(&format!("    {}", meta.description))?;
            fmt.newline()?;
            fmt.newline()?;
        }
    }

    Ok(())
}

fn print_help_markdown(
    section_filter: &Option<String>,
    reg: &crate::config::ConfigRegistry,
) -> io::Result<()> {
    let mut stdout = io::stdout();

    writeln!(stdout, "# Cassy Configuration Reference")?;
    writeln!(stdout)?;
    writeln!(stdout, "Use `cas config <key> <value>` to set options.")?;
    writeln!(
        stdout,
        "Use `cas config describe <key>` for detailed info on any option."
    )?;
    writeln!(stdout)?;

    for section in reg.sections() {
        if let Some(filter) = section_filter {
            if !section.starts_with(filter.as_str()) {
                continue;
            }
        }

        let section_desc = reg.section_description(section).unwrap_or("");
        writeln!(stdout, "## {section}")?;
        writeln!(stdout)?;
        writeln!(stdout, "{section_desc}")?;
        writeln!(stdout)?;
        writeln!(stdout, "| Key | Type | Default | Description |")?;
        writeln!(stdout, "|-----|------|---------|-------------|")?;

        for meta in reg.configs_in_section(section) {
            let desc_short = if meta.description.len() > 50 {
                format!("{}...", &meta.description[..47])
            } else {
                meta.description.to_string()
            };
            writeln!(
                stdout,
                "| `{}` | {} | `{}` | {} |",
                meta.key,
                meta.value_type.name(),
                meta.default,
                desc_short
            )?;
        }
        writeln!(stdout)?;
    }

    Ok(())
}

fn print_help_man(
    section_filter: &Option<String>,
    reg: &crate::config::ConfigRegistry,
) -> io::Result<()> {
    let mut stdout = io::stdout();

    writeln!(
        stdout,
        ".TH Cassy-CONFIG 7 \"2024\" \"Cassy\" \"Configuration Reference\""
    )?;
    writeln!(stdout, ".SH NAME")?;
    writeln!(stdout, "cas-config \\- Cassy configuration options")?;
    writeln!(stdout, ".SH DESCRIPTION")?;
    writeln!(
        stdout,
        "This manual page documents the configuration options for Cassy."
    )?;
    writeln!(stdout)?;

    for section in reg.sections() {
        if let Some(filter) = section_filter {
            if !section.starts_with(filter.as_str()) {
                continue;
            }
        }

        let section_desc = reg.section_description(section).unwrap_or("");
        writeln!(stdout, ".SH {}", section.to_uppercase().replace('.', " "))?;
        writeln!(stdout, "{section_desc}")?;

        for meta in reg.configs_in_section(section) {
            writeln!(stdout, ".TP")?;
            writeln!(stdout, ".B {}", meta.key)?;
            writeln!(stdout, "{}", meta.description)?;
            writeln!(stdout, ".br")?;
            writeln!(
                stdout,
                "Type: {} | Default: {}",
                meta.value_type.name(),
                meta.default
            )?;
        }
        writeln!(stdout)?;
    }

    Ok(())
}

pub(crate) fn execute_completions(args: &ConfigCompletionsArgs, _cli: &Cli) -> anyhow::Result<()> {
    let reg = registry();
    let keys: Vec<&str> = reg.all_keys();
    let mut stdout = io::stdout();

    match args.shell {
        CompletionShell::Bash => {
            writeln!(stdout, "# Bash completion for cas config")?;
            writeln!(stdout, "# Add to ~/.bashrc or ~/.bash_completion")?;
            writeln!(stdout)?;
            writeln!(stdout, "_cas_config_keys() {{")?;
            writeln!(stdout, "    local keys=\"{}\"", keys.join(" "))?;
            writeln!(
                stdout,
                "    COMPREPLY=($(compgen -W \"$keys\" -- \"${{COMP_WORDS[COMP_CWORD]}}\"))"
            )?;
            writeln!(stdout, "}}")?;
            writeln!(stdout)?;
            writeln!(stdout, "_cas_config() {{")?;
            writeln!(stdout, "    local cur prev")?;
            writeln!(stdout, "    cur=\"${{COMP_WORDS[COMP_CWORD]}}\"")?;
            writeln!(stdout, "    prev=\"${{COMP_WORDS[COMP_CWORD-1]}}\"")?;
            writeln!(stdout)?;
            writeln!(stdout, "    case \"$prev\" in")?;
            writeln!(stdout, "        config)")?;
            writeln!(
                stdout,
                "            COMPREPLY=($(compgen -W \"get set list describe diff reset export import search docs completions\" -- \"$cur\"))"
            )?;
            writeln!(stdout, "            return 0")?;
            writeln!(stdout, "            ;;")?;
            writeln!(stdout, "        get|set|describe|reset)")?;
            writeln!(stdout, "            _cas_config_keys")?;
            writeln!(stdout, "            return 0")?;
            writeln!(stdout, "            ;;")?;
            writeln!(stdout, "    esac")?;
            writeln!(stdout, "}}")?;
            writeln!(stdout)?;
            writeln!(stdout, "complete -F _cas_config cas config")?;
        }
        CompletionShell::Zsh => {
            writeln!(stdout, "# Zsh completion for cas config")?;
            writeln!(stdout, "# Add to ~/.zshrc or a file in $fpath")?;
            writeln!(stdout)?;
            writeln!(stdout, "#compdef cas")?;
            writeln!(stdout)?;
            writeln!(stdout, "_cas_config_keys() {{")?;
            writeln!(stdout, "    local keys=(")?;
            for key in &keys {
                if let Some(meta) = reg.get(key) {
                    writeln!(stdout, "        '{}[{}]'", key, meta.name)?;
                }
            }
            writeln!(stdout, "    )")?;
            writeln!(stdout, "    _describe 'config key' keys")?;
            writeln!(stdout, "}}")?;
            writeln!(stdout)?;
            writeln!(stdout, "_cas_config() {{")?;
            writeln!(stdout, "    local -a subcmds")?;
            writeln!(stdout, "    subcmds=(")?;
            writeln!(stdout, "        'get:Get a config value'")?;
            writeln!(stdout, "        'set:Set a config value'")?;
            writeln!(stdout, "        'list:List all config options'")?;
            writeln!(
                stdout,
                "        'describe:Show detailed information about a config option'"
            )?;
            writeln!(
                stdout,
                "        'diff:Show differences from default configuration'"
            )?;
            writeln!(stdout, "        'reset:Reset config option(s) to default'")?;
            writeln!(stdout, "        'export:Export configuration to file'")?;
            writeln!(stdout, "        'import:Import configuration from file'")?;
            writeln!(stdout, "        'search:Search config options by keyword'")?;
            writeln!(
                stdout,
                "        'docs:Show full configuration documentation'"
            )?;
            writeln!(
                stdout,
                "        'completions:Generate shell completion scripts'"
            )?;
            writeln!(stdout, "    )")?;
            writeln!(stdout)?;
            writeln!(stdout, "    case $CURRENT in")?;
            writeln!(stdout, "        2)")?;
            writeln!(stdout, "            _describe 'subcommand' subcmds")?;
            writeln!(stdout, "            ;;")?;
            writeln!(stdout, "        3)")?;
            writeln!(stdout, "            case $words[2] in")?;
            writeln!(stdout, "                get|set|describe|reset)")?;
            writeln!(stdout, "                    _cas_config_keys")?;
            writeln!(stdout, "                    ;;")?;
            writeln!(stdout, "            esac")?;
            writeln!(stdout, "            ;;")?;
            writeln!(stdout, "    esac")?;
            writeln!(stdout, "}}")?;
        }
        CompletionShell::Fish => {
            writeln!(stdout, "# Fish completion for cas config")?;
            writeln!(stdout, "# Save to ~/.config/fish/completions/cas.fish")?;
            writeln!(stdout)?;
            writeln!(stdout, "# Config subcommands")?;
            writeln!(
                stdout,
                "complete -c cas -n '__fish_seen_subcommand_from config' -a 'get set list describe diff reset export import search docs completions' -d 'Config subcommand'"
            )?;
            writeln!(stdout)?;
            writeln!(stdout, "# Config keys for get/set/describe/reset")?;
            for key in &keys {
                if let Some(meta) = reg.get(key) {
                    let desc_short = if meta.description.len() > 40 {
                        format!("{}...", &meta.description[..37])
                    } else {
                        meta.description.to_string()
                    };
                    writeln!(
                        stdout,
                        "complete -c cas -n '__fish_seen_subcommand_from config; and __fish_seen_subcommand_from get set describe reset' -a '{}' -d '{}'",
                        key,
                        desc_short.replace('\'', "\\'")
                    )?;
                }
            }
        }
    }

    Ok(())
}
