//! MCP CLI commands for managing upstream MCP server configurations.
//!
//! Mirrors cmcp's CLI interface so users can copy-paste `claude mcp add` or
//! `codex mcp add` commands by prepending `cas mcp`.
//!
//! Config is stored in `.cas/proxy.toml` (project-scoped). Servers are exposed
//! via `mcp_search` and `mcp_execute` tools in `cas serve`.

use std::path::Path;

use anyhow::Result;
use clap::Subcommand;

#[cfg(feature = "mcp-proxy")]
use std::collections::HashMap;
#[cfg(feature = "mcp-proxy")]
use std::io::{self, Write};
#[cfg(feature = "mcp-proxy")]
use std::path::PathBuf;
#[cfg(feature = "mcp-proxy")]
use anyhow::Context;
#[cfg(feature = "mcp-proxy")]
use cmcp_core::config::{Config, ServerConfig};
#[cfg(feature = "mcp-proxy")]
use crate::ui::components::{Formatter, Renderable, StatusLine};
#[cfg(feature = "mcp-proxy")]
use crate::ui::theme::ActiveTheme;
#[cfg(not(feature = "mcp-proxy"))]
use anyhow::bail;

#[derive(Subcommand)]
pub enum McpCommands {
    /// Add an MCP server (drop-in replacement for `claude mcp add`).
    ///
    /// Copy any `claude mcp add` command and replace `claude mcp` with `cas mcp`:
    ///
    ///   cas mcp add chrome-devtools --scope user npx chrome-devtools-mcp@latest
    ///   cas mcp add -e API_KEY=xxx my-server -- npx my-mcp-server --some-flag
    ///   cas mcp add --transport http sentry https://mcp.sentry.dev/mcp
    ///   cas mcp add -H "Authorization: Bearer ..." corridor https://app.corridor.dev/api/mcp
    ///   cas mcp add canva https://mcp.canva.com/mcp
    Add {
        /// Raw arguments — parsed identically to `claude mcp add`.
        /// Supports: -s/--scope, -t/--transport, -e/--env, -H/--header, --auth
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        raw: Vec<String>,
    },

    /// Remove an MCP server.
    Remove {
        /// Server name to remove
        name: String,

        /// Scope: "local" (default), "user", or "project".
        #[arg(short, long, default_value = "local")]
        scope: String,
    },

    /// List configured MCP servers.
    #[command(alias = "ls")]
    List {
        /// Only show server names (don't connect to fetch tools)
        #[arg(short, long)]
        short: bool,

        /// Show secret values (stdio env, http/sse auth + headers) in plaintext.
        /// Off by default — `cas mcp list --json` redacts secrets unless this is
        /// set (cas-9f07: a tool-less Codex worker leaked live tokens via --json).
        #[arg(long)]
        show_secrets: bool,
    },

    /// Import MCP servers from Claude or Codex config.
    ///
    /// Examples:
    ///   cas mcp import                    # import from all sources
    ///   cas mcp import --from claude      # only from Claude
    ///   cas mcp import --from codex       # only from Codex
    ///   cas mcp import --dry-run          # preview without writing
    ///   cas mcp import --force            # overwrite existing servers
    Import {
        /// Source to import from: "claude", "codex", or omit for all.
        #[arg(short, long)]
        from: Option<String>,

        /// Preview what would be imported without writing.
        #[arg(short, long)]
        dry_run: bool,

        /// Overwrite existing servers with the same name.
        #[arg(long)]
        force: bool,
    },
}

#[cfg(feature = "mcp-proxy")]
pub fn execute(cmd: &McpCommands, cli: &super::Cli, cas_root: &Path) -> Result<()> {
    match cmd {
        McpCommands::Add { raw } => execute_add(raw, cli, cas_root),
        McpCommands::Remove { name, scope } => execute_remove(name, scope, cli, cas_root),
        McpCommands::List { short, show_secrets } => {
            execute_list(*short, *show_secrets, cli, cas_root)
        }
        McpCommands::Import {
            from,
            dry_run,
            force,
        } => execute_import(from.as_deref(), *dry_run, *force, cli, cas_root),
    }
}

#[cfg(not(feature = "mcp-proxy"))]
pub fn execute(_cmd: &McpCommands, _cli: &super::Cli, _cas_root: &Path) -> Result<()> {
    bail!("MCP proxy commands require the 'mcp-proxy' feature. Build with: cargo build --features mcp-proxy")
}

#[cfg(feature = "mcp-proxy")]
fn resolve_scope_path(scope: &str, cas_root: &Path) -> Result<PathBuf> {
    match scope {
        "user" | "global" => cmcp_core::config::Scope::User
            .config_path()
            .context("could not determine user config path"),
        _ => Ok(cas_root.join("proxy.toml")),
    }
}

#[cfg(feature = "mcp-proxy")]
fn project_config_path(cas_root: &Path) -> PathBuf {
    cas_root.join("proxy.toml")
}

#[cfg(feature = "mcp-proxy")]
fn load_config_for_scope(scope: &str, cas_root: &Path) -> Result<(Config, PathBuf)> {
    let path = resolve_scope_path(scope, cas_root)?;
    let config = Config::load_from(&path)?;
    Ok((config, path))
}

#[cfg(feature = "mcp-proxy")]
fn load_config(cas_root: &Path) -> Result<Config> {
    let path = project_config_path(cas_root);
    Config::load_from(&path)
}

// ── Add ──────────────────────────────────────────────────────────────

/// Manually parse raw args from `cas mcp add`, matching `claude mcp add` syntax.
///
/// Supports all Claude flags: -s/--scope, -t/--transport, -e/--env, -H/--header
/// Plus CAS extras: -a/--auth
/// Ignores Claude-only flags: --callback-port, --client-id, --client-secret
///
/// After flags, first positional = server name, rest = url/command + args.
/// Everything after `--` is passed literally to the subprocess.
#[cfg(feature = "mcp-proxy")]
fn execute_add(raw: &[String], cli: &super::Cli, cas_root: &Path) -> Result<()> {
    let mut transport: Option<String> = None;
    let mut auth: Option<String> = None;
    let mut headers: Vec<String> = Vec::new();
    let mut envs: Vec<String> = Vec::new();
    let mut scope = "local".to_string();
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;

    while i < raw.len() {
        let arg = &raw[i];

        // -- terminates flag parsing; everything after is literal.
        if arg == "--" {
            positional.push(arg.clone());
            positional.extend(raw[i + 1..].iter().cloned());
            break;
        }

        match arg.as_str() {
            "--scope" | "-s" if i + 1 < raw.len() => {
                scope = raw[i + 1].clone();
                i += 2;
            }
            "--transport" | "-t" if i + 1 < raw.len() => {
                transport = Some(raw[i + 1].clone());
                i += 2;
            }
            "--auth" | "-a" if i + 1 < raw.len() => {
                auth = Some(raw[i + 1].clone());
                i += 2;
            }
            "--header" | "-H" if i + 1 < raw.len() => {
                headers.push(raw[i + 1].clone());
                i += 2;
            }
            "--env" | "-e" if i + 1 < raw.len() => {
                envs.push(raw[i + 1].clone());
                i += 2;
            }
            // Claude-specific OAuth flags — consume and ignore.
            "--callback-port" | "--client-id" if i + 1 < raw.len() => {
                i += 2;
            }
            "--client-secret" => {
                i += 1;
            }
            // Codex-specific flags — consume and ignore.
            "--bearer-token" | "--bearer-token-env-var" if i + 1 < raw.len() => {
                i += 2;
            }
            _ => {
                positional.push(arg.clone());
                i += 1;
            }
        }
    }

    // First positional = server name, rest = url/command + args.
    let name = positional
        .first()
        .context("missing server name. Usage: cas mcp add <name> <url-or-command>")?
        .clone();
    let cmd_args: Vec<String> = positional[1..].to_vec();

    // Auto-detect transport from the first non-flag arg after the name.
    let first_cmd_arg = cmd_args
        .iter()
        .find(|a| a.as_str() != "--")
        .map(|s| s.as_str());

    let transport = transport.unwrap_or_else(|| {
        if let Some(arg) = first_cmd_arg {
            if arg.starts_with("http://") || arg.starts_with("https://") {
                "http".to_string()
            } else {
                "stdio".to_string()
            }
        } else {
            "http".to_string()
        }
    });

    // Build ServerConfig based on transport.
    let server_config = match transport.as_str() {
        "http" => {
            let url = first_cmd_arg
                .context("missing URL. Usage: cas mcp add <name> <url>")?
                .to_string();
            ServerConfig::Http {
                url,
                auth,
                headers: parse_headers(&headers),
                oauth: false,
            }
        }
        "sse" => {
            let url = first_cmd_arg
                .context("missing URL. Usage: cas mcp add --transport sse <name> <url>")?
                .to_string();
            ServerConfig::Sse {
                url,
                auth,
                headers: parse_headers(&headers),
                oauth: false,
            }
        }
        "stdio" => {
            // Skip leading -- separators.
            let cleaned: Vec<String> = cmd_args
                .iter()
                .skip_while(|a| a.as_str() == "--")
                .cloned()
                .collect();

            let command = cleaned
                .first()
                .context("missing command. Usage: cas mcp add <name> -- <command> [args...]")?
                .clone();

            let args = cleaned.get(1..).unwrap_or_default().to_vec();

            ServerConfig::Stdio {
                command,
                args,
                env: parse_envs(&envs),
            }
        }
        other => anyhow::bail!("unknown transport \"{other}\". Use: http, stdio, or sse"),
    };

    let transport_name = match &server_config {
        ServerConfig::Http { .. } => "http",
        ServerConfig::Sse { .. } => "sse",
        ServerConfig::Stdio { .. } => "stdio",
    };

    let (mut config, path) = load_config_for_scope(&scope, cas_root)?;
    let is_update = config.servers.contains_key(&name);
    config.add_server(name.clone(), server_config);
    config.save_to(&path)?;
    let public_name = cas_types::public_upstream_id(&name);

    if cli.json {
        println!(
            "{}",
            serde_json::json!({
                "action": if is_update { "updated" } else { "added" },
                "name": public_name,
                "transport": transport_name,
            })
        );
    } else {
        let theme = ActiveTheme::default();
        let mut stdout = io::stdout();
        let mut fmt = Formatter::stdout(&mut stdout, theme);
        let verb = if is_update { "Updated" } else { "Added" };
        StatusLine::success(format!("{verb} server \"{public_name}\"")).render(&mut fmt)?;
        fmt.field("Config", &path.display().to_string())?;
    }

    Ok(())
}

/// Parse "Key: Value" or "Key=Value" header strings.
#[cfg(feature = "mcp-proxy")]
fn parse_headers(raw: &[String]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for h in raw {
        if let Some((k, v)) = h.split_once(':') {
            map.insert(k.trim().to_string(), v.trim().to_string());
        } else if let Some((k, v)) = h.split_once('=') {
            map.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    map
}

/// Parse "KEY=VALUE" env strings.
#[cfg(feature = "mcp-proxy")]
fn parse_envs(raw: &[String]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for e in raw {
        if let Some((k, v)) = e.split_once('=') {
            map.insert(k.to_string(), v.to_string());
        }
    }
    map
}

// ── Remove ───────────────────────────────────────────────────────────

#[cfg(feature = "mcp-proxy")]
fn execute_remove(name: &str, scope: &str, cli: &super::Cli, cas_root: &Path) -> Result<()> {
    let (mut config, path) = load_config_for_scope(scope, cas_root)?;
    let public_name = cas_types::public_upstream_id(name);

    if !config.remove_server(name) {
        if cli.json {
            println!(
                "{}",
                serde_json::json!({ "error": format!("Server '{public_name}' not found") })
            );
        } else {
            let theme = ActiveTheme::default();
            let mut stdout = io::stdout();
            let mut fmt = Formatter::stdout(&mut stdout, theme);
            StatusLine::warning(format!("Server \"{public_name}\" not found")).render(&mut fmt)?;
        }
        return Ok(());
    }

    config.save_to(&path)?;

    if cli.json {
        println!(
            "{}",
            serde_json::json!({ "action": "removed", "name": public_name })
        );
    } else {
        let theme = ActiveTheme::default();
        let mut stdout = io::stdout();
        let mut fmt = Formatter::stdout(&mut stdout, theme);
        StatusLine::success(format!("Removed server \"{public_name}\"")).render(&mut fmt)?;
    }

    Ok(())
}

// ── List ─────────────────────────────────────────────────────────────

/// Marker substituted for secret values in `cas mcp list` output unless
/// `--show-secrets` is passed (cas-9f07).
#[cfg(feature = "mcp-proxy")]
const SECRET_REDACTION: &str = "***REDACTED***";

/// Redact the secret-bearing fields of a serialized `ServerConfig` JSON value
/// in place (cas-9f07). `ServerConfig` carries stdio `env`, http/sse `auth`,
/// and `headers` — all of which `serde_json::to_value` emits verbatim. A
/// tool-less Codex worker that improvised `cas mcp list --json` leaked a live
/// GitHub PAT, Neon API key, and Vercel token into its rollout log because of
/// this. We replace each secret *value* with [`SECRET_REDACTION`] while leaving
/// keys, projected server identities, commands, args, URLs, and `oauth`
/// visible so the listing still tells you which server holds which credential
/// without echoing an unsafe raw config key.
#[cfg(feature = "mcp-proxy")]
fn redact_server_secrets(value: &mut serde_json::Value) {
    let serde_json::Value::Object(map) = value else {
        return;
    };
    // http/sse `auth: Option<String>` — redact when present and non-null.
    if let Some(auth) = map.get_mut("auth") {
        if auth.is_string() {
            *auth = serde_json::Value::String(SECRET_REDACTION.to_string());
        }
    }
    // stdio `env` and http/sse `headers` — redact every value, keep the keys.
    for field in ["env", "headers"] {
        if let Some(serde_json::Value::Object(inner)) = map.get_mut(field) {
            for v in inner.values_mut() {
                *v = serde_json::Value::String(SECRET_REDACTION.to_string());
            }
        }
    }
}

#[cfg(feature = "mcp-proxy")]
fn execute_list(short: bool, show_secrets: bool, cli: &super::Cli, cas_root: &Path) -> Result<()> {
    use crate::ui::components::Table;

    // Merge project config (.cas/proxy.toml) with user config (~/.config/code-mode-mcp/config.toml)
    let project_path = project_config_path(cas_root);
    let config = Config::load_merged(if project_path.exists() {
        Some(&project_path)
    } else {
        None
    })?;

    if config.servers.is_empty() {
        if cli.json {
            println!("{}", serde_json::json!({ "servers": [] }));
        } else {
            let theme = ActiveTheme::default();
            let mut stdout = io::stdout();
            let mut fmt = Formatter::stdout(&mut stdout, theme);
            StatusLine::info("No MCP servers configured.").render(&mut fmt)?;
            fmt.info("Add one with: cas mcp add <name> <url-or-command>")?;
        }
        return Ok(());
    }

    if cli.json {
        let public_names =
            cas_types::public_upstream_ids(config.servers.keys().map(String::as_str));
        let servers: Vec<serde_json::Value> = config
            .servers
            .iter()
            .map(|(name, cfg)| {
                let mut obj = serde_json::to_value(cfg).unwrap_or_default();
                if !show_secrets {
                    redact_server_secrets(&mut obj);
                }
                if let serde_json::Value::Object(ref mut m) = obj {
                    m.insert(
                        "name".to_string(),
                        serde_json::json!(
                            public_names
                                .get(name)
                                .cloned()
                                .unwrap_or_else(|| cas_types::public_upstream_id(name))
                        ),
                    );
                }
                obj
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "servers": servers }))?
        );
        return Ok(());
    }

    let theme = ActiveTheme::default();
    let mut stdout = io::stdout();
    let mut fmt = Formatter::stdout(&mut stdout, theme);

    let public_names = cas_types::public_upstream_ids(config.servers.keys().map(String::as_str));
    let rows: Vec<Vec<String>> = config
        .servers
        .iter()
        .map(|(name, cfg)| {
            let (transport, detail) = match cfg {
                ServerConfig::Http { url, .. } => ("http", url.clone()),
                ServerConfig::Sse { url, .. } => ("sse", url.clone()),
                ServerConfig::Stdio {
                    command, args: a, ..
                } => {
                    let args_str = if a.is_empty() {
                        String::new()
                    } else {
                        format!(" {}", a.join(" "))
                    };
                    ("stdio", format!("{command}{args_str}"))
                }
            };
            vec![
                public_names
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| cas_types::public_upstream_id(name)),
                transport.to_string(),
                detail,
            ]
        })
        .collect();

    if !short {
        fmt.info(&format!("{} MCP server(s):", config.servers.len()))?;
        fmt.newline()?;
    }

    Table::new()
        .columns(&["NAME", "TRANSPORT", "ENDPOINT"])
        .rows(rows)
        .render(&mut fmt)?;

    Ok(())
}

// ── Import ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(feature = "mcp-proxy")]
enum ImportSource {
    Claude,
    Codex,
}

#[cfg(feature = "mcp-proxy")]
impl std::fmt::Display for ImportSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportSource::Claude => write!(f, "claude"),
            ImportSource::Codex => write!(f, "codex"),
        }
    }
}

#[cfg(feature = "mcp-proxy")]
struct ImportedServer {
    name: String,
    config: ServerConfig,
    source: ImportSource,
}

#[cfg(feature = "mcp-proxy")]
fn execute_import(
    from: Option<&str>,
    dry_run: bool,
    force: bool,
    cli: &super::Cli,
    cas_root: &Path,
) -> Result<()> {
    let source_filter = match from {
        Some("claude" | "claude-code") => Some(ImportSource::Claude),
        Some("codex" | "openai") => Some(ImportSource::Codex),
        Some(other) => {
            anyhow::bail!("unknown source \"{other}\". Use: claude, codex, or omit for all")
        }
        None => None,
    };

    let discovered = discover_servers(source_filter)?;

    if discovered.is_empty() {
        if cli.json {
            println!("{}", serde_json::json!({ "imported": 0 }));
        } else {
            let theme = ActiveTheme::default();
            let mut stdout = io::stdout();
            let mut fmt = Formatter::stdout(&mut stdout, theme);
            StatusLine::info("No MCP servers found to import.").render(&mut fmt)?;
            if source_filter.is_none() {
                fmt.newline()?;
                fmt.info("Searched:")?;
                fmt.bullet("Claude: ~/.claude.json, .mcp.json")?;
                fmt.bullet("Codex:  ~/.codex/config.toml, .codex/config.toml")?;
            }
        }
        return Ok(());
    }

    let theme = ActiveTheme::default();
    let mut stdout = io::stdout();
    let mut fmt = Formatter::stdout(&mut stdout, theme);

    let mut config = load_config(cas_root)?;
    let mut added = 0usize;
    let mut skipped = 0usize;
    let mut updated = 0usize;
    let public_discovered =
        cas_types::public_upstream_ids(discovered.iter().map(|server| server.name.as_str()));

    for server in &discovered {
        let exists = config.servers.contains_key(&server.name);
        let public_name = public_discovered
            .get(&server.name)
            .cloned()
            .unwrap_or_else(|| cas_types::public_upstream_id(&server.name));

        let transport_info = match &server.config {
            ServerConfig::Http { url, .. } => format!("http  {url}"),
            ServerConfig::Sse { url, .. } => format!("sse   {url}"),
            ServerConfig::Stdio {
                command, args: a, ..
            } => format!("stdio {} {}", command, a.join(" ")),
        };

        if exists && !force {
            if dry_run {
                fmt.bullet(&format!(
                    "skip  {:<20} {:<12} {} (already exists)",
                    public_name, server.source, transport_info
                ))?;
            }
            skipped += 1;
        } else if exists && force {
            if dry_run {
                fmt.bullet(&format!(
                    "update {:<19} {:<12} {}",
                    public_name, server.source, transport_info
                ))?;
            } else {
                config.add_server(server.name.clone(), server.config.clone());
            }
            updated += 1;
        } else {
            if dry_run {
                fmt.bullet(&format!(
                    "add   {:<20} {:<12} {}",
                    public_name, server.source, transport_info
                ))?;
            } else {
                config.add_server(server.name.clone(), server.config.clone());
            }
            added += 1;
        }
    }

    if dry_run {
        fmt.newline()?;
        StatusLine::info(format!(
            "Dry run: {} to add, {} to update, {} to skip",
            added, updated, skipped
        ))
        .render(&mut fmt)?;
        fmt.info("Run without --dry-run to apply.")?;
    } else {
        let path = project_config_path(cas_root);
        config.save_to(&path)?;

        if cli.json {
            let names: Vec<String> = discovered
                .iter()
                .filter(|s| !config.servers.contains_key(&s.name) || force)
                .map(|server| {
                    public_discovered
                        .get(&server.name)
                        .cloned()
                        .unwrap_or_else(|| cas_types::public_upstream_id(&server.name))
                })
                .collect();
            println!(
                "{}",
                serde_json::json!({
                    "imported": added + updated,
                    "added": added,
                    "updated": updated,
                    "skipped": skipped,
                    "servers": names,
                })
            );
        } else if added > 0 || updated > 0 {
            StatusLine::success(format!(
                "Imported {} server(s) ({} added, {} updated, {} skipped)",
                added + updated,
                added,
                updated,
                skipped
            ))
            .render(&mut fmt)?;
            fmt.field(
                "Config",
                &project_config_path(cas_root).display().to_string(),
            )?;
        } else {
            StatusLine::info(format!(
                "No new servers to import ({} already exist).",
                skipped
            ))
            .render(&mut fmt)?;
        }
    }

    Ok(())
}

// ── Import discovery ─────────────────────────────────────────────────

#[cfg(feature = "mcp-proxy")]
fn discover_servers(source_filter: Option<ImportSource>) -> Result<Vec<ImportedServer>> {
    let mut servers = Vec::new();

    if source_filter.is_none() || source_filter == Some(ImportSource::Claude) {
        servers.extend(discover_claude()?);
    }

    if source_filter.is_none() || source_filter == Some(ImportSource::Codex) {
        servers.extend(discover_codex()?);
    }

    Ok(servers)
}

#[cfg(feature = "mcp-proxy")]
fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME not set")
}

// ── Claude discovery ─────────────────────────────────────────────────

#[cfg(feature = "mcp-proxy")]
fn discover_claude() -> Result<Vec<ImportedServer>> {
    let mut servers = Vec::new();
    let home = home_dir()?;

    // User-scoped: ~/.claude.json
    let user_config = home.join(".claude.json");
    if user_config.exists() {
        servers.extend(parse_claude_json(&user_config)?);
    }

    // Project-scoped: .mcp.json
    let project_config = PathBuf::from(".mcp.json");
    if project_config.exists() {
        servers.extend(parse_claude_json(&project_config)?);
    }

    Ok(servers)
}

#[cfg(feature = "mcp-proxy")]
fn parse_claude_json(path: &Path) -> Result<Vec<ImportedServer>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;

    let root: serde_json::Value = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse {}", path.display()))?;

    let Some(mcp_servers) = root.get("mcpServers").and_then(|v| v.as_object()) else {
        return Ok(Vec::new());
    };

    let mut servers = Vec::new();
    for (name, value) in mcp_servers {
        match parse_claude_server(name, value) {
            Ok(Some(server)) => servers.push(server),
            Ok(None) => {}
            Err(e) => {
                let _ = writeln!(
                    io::stderr(),
                    "  warning: skipping {}: {e}",
                    cas_types::public_upstream_id(name)
                );
            }
        }
    }
    Ok(servers)
}

#[cfg(feature = "mcp-proxy")]
fn parse_claude_server(name: &str, value: &serde_json::Value) -> Result<Option<ImportedServer>> {
    let obj = value
        .as_object()
        .context("server config is not an object")?;

    let transport = obj.get("type").and_then(|v| v.as_str()).unwrap_or("stdio");

    let config = match transport {
        "stdio" => {
            let command = obj
                .get("command")
                .and_then(|v| v.as_str())
                .context("missing command")?
                .to_string();

            let args = obj
                .get("args")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            let env = parse_json_string_map(obj.get("env"));

            ServerConfig::Stdio { command, args, env }
        }
        "http" => {
            let url = obj
                .get("url")
                .and_then(|v| v.as_str())
                .context("missing url")?
                .to_string();

            let headers = parse_json_string_map(obj.get("headers"));
            let (auth, headers) = extract_auth_header(headers);

            ServerConfig::Http {
                url,
                auth,
                headers,
                oauth: false,
            }
        }
        "sse" => {
            let url = obj
                .get("url")
                .and_then(|v| v.as_str())
                .context("missing url")?
                .to_string();

            let headers = parse_json_string_map(obj.get("headers"));
            let (auth, headers) = extract_auth_header(headers);

            ServerConfig::Sse {
                url,
                auth,
                headers,
                oauth: false,
            }
        }
        _ => return Ok(None),
    };

    Ok(Some(ImportedServer {
        name: name.to_string(),
        config,
        source: ImportSource::Claude,
    }))
}

// ── Codex discovery ──────────────────────────────────────────────────

#[cfg(feature = "mcp-proxy")]
fn discover_codex() -> Result<Vec<ImportedServer>> {
    let mut servers = Vec::new();
    let home = home_dir()?;

    // User-scoped: ~/.codex/config.toml
    let user_config = home.join(".codex").join("config.toml");
    if user_config.exists() {
        servers.extend(parse_codex_toml(&user_config)?);
    }

    // Project-scoped: .codex/config.toml
    let project_config = PathBuf::from(".codex").join("config.toml");
    if project_config.exists() {
        servers.extend(parse_codex_toml(&project_config)?);
    }

    Ok(servers)
}

#[cfg(feature = "mcp-proxy")]
fn parse_codex_toml(path: &Path) -> Result<Vec<ImportedServer>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;

    let root: toml::Value = content
        .parse()
        .with_context(|| format!("failed to parse {}", path.display()))?;

    let Some(mcp_servers) = root.get("mcp_servers").and_then(|v| v.as_table()) else {
        return Ok(Vec::new());
    };

    let mut servers = Vec::new();
    for (name, value) in mcp_servers {
        match parse_codex_server(name, value) {
            Ok(Some(server)) => servers.push(server),
            Ok(None) => {}
            Err(e) => {
                let _ = writeln!(
                    io::stderr(),
                    "  warning: skipping {}: {e}",
                    cas_types::public_upstream_id(name)
                );
            }
        }
    }
    Ok(servers)
}

#[cfg(feature = "mcp-proxy")]
fn parse_codex_server(name: &str, value: &toml::Value) -> Result<Option<ImportedServer>> {
    let table = value.as_table().context("server config is not a table")?;

    // Skip disabled servers.
    if let Some(enabled) = table.get("enabled").and_then(|v| v.as_bool()) {
        if !enabled {
            return Ok(None);
        }
    }

    let has_url = table.get("url").is_some();
    let has_command = table.get("command").is_some();

    let config = if has_url {
        let url = table
            .get("url")
            .and_then(|v| v.as_str())
            .context("missing url")?
            .to_string();

        let auth = if let Some(env_var) = table.get("bearer_token_env_var").and_then(|v| v.as_str())
        {
            Some(format!("env:{env_var}"))
        } else {
            table
                .get("bearer_token")
                .and_then(|v| v.as_str())
                .map(String::from)
        };

        let mut headers = parse_toml_string_map(table.get("http_headers"));
        if let Some(env_headers) = table.get("env_http_headers").and_then(|v| v.as_table()) {
            for (k, v) in env_headers {
                if let Some(env_var) = v.as_str() {
                    headers.insert(k.clone(), format!("env:{env_var}"));
                }
            }
        }

        ServerConfig::Http {
            url,
            auth,
            headers,
            oauth: false,
        }
    } else if has_command {
        let command = table
            .get("command")
            .and_then(|v| v.as_str())
            .context("missing command")?
            .to_string();

        let args = table
            .get("args")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let mut env = parse_toml_string_map(table.get("env"));
        if let Some(env_vars) = table.get("env_vars").and_then(|v| v.as_array()) {
            for var in env_vars {
                if let Some(var_name) = var.as_str() {
                    env.insert(var_name.to_string(), format!("env:{var_name}"));
                }
            }
        }

        ServerConfig::Stdio { command, args, env }
    } else {
        anyhow::bail!("server has neither 'url' nor 'command'")
    };

    Ok(Some(ImportedServer {
        name: name.to_string(),
        config,
        source: ImportSource::Codex,
    }))
}

// ── Helpers ──────────────────────────────────────────────────────────

#[cfg(feature = "mcp-proxy")]
fn parse_json_string_map(value: Option<&serde_json::Value>) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let Some(obj) = value.and_then(|v| v.as_object()) {
        for (k, v) in obj {
            if let Some(s) = v.as_str() {
                map.insert(k.clone(), s.to_string());
            }
        }
    }
    map
}

#[cfg(feature = "mcp-proxy")]
fn parse_toml_string_map(value: Option<&toml::Value>) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let Some(table) = value.and_then(|v| v.as_table()) {
        for (k, v) in table {
            if let Some(s) = v.as_str() {
                map.insert(k.clone(), s.to_string());
            }
        }
    }
    map
}

#[cfg(feature = "mcp-proxy")]
fn extract_auth_header(
    mut headers: HashMap<String, String>,
) -> (Option<String>, HashMap<String, String>) {
    let auth = headers
        .remove("Authorization")
        .or_else(|| headers.remove("authorization"))
        .and_then(|v| {
            if let Some(token) = v.strip_prefix("Bearer ") {
                Some(token.to_string())
            } else if let Some(token) = v.strip_prefix("bearer ") {
                Some(token.to_string())
            } else {
                headers.insert("Authorization".to_string(), v);
                None
            }
        });
    (auth, headers)
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(all(test, feature = "mcp-proxy"))]
mod tests {
    use super::*;
    use cmcp_core::config::ServerConfig;
    use std::collections::HashMap;

    /// cas-9f07: stdio `env` values are redacted by default; keys + command stay.
    #[test]
    fn redacts_stdio_env_values_by_default() {
        let cfg = ServerConfig::Stdio {
            command: "cas".into(),
            args: vec!["serve".into()],
            env: HashMap::from([("GITHUB_PAT".to_string(), "ghp_SECRET123".to_string())]),
        };
        let mut v = serde_json::to_value(&cfg).unwrap();
        redact_server_secrets(&mut v);
        let s = serde_json::to_string(&v).unwrap();
        assert!(!s.contains("ghp_SECRET123"), "env secret must be redacted: {s}");
        assert!(s.contains(SECRET_REDACTION), "redaction marker expected: {s}");
        assert!(s.contains("GITHUB_PAT"), "env key should stay visible: {s}");
        assert!(s.contains("\"cas\""), "command should stay visible: {s}");
    }

    /// cas-9f07: http `auth` and `headers` values are redacted; url stays.
    #[test]
    fn redacts_http_auth_and_headers() {
        let cfg = ServerConfig::Http {
            url: "https://mcp.example.com".into(),
            auth: Some("bearer_SECRET456".into()),
            headers: HashMap::from([("X-Api-Key".to_string(), "key_SECRET789".to_string())]),
            oauth: false,
        };
        let mut v = serde_json::to_value(&cfg).unwrap();
        redact_server_secrets(&mut v);
        let s = serde_json::to_string(&v).unwrap();
        assert!(!s.contains("bearer_SECRET456"), "auth must be redacted: {s}");
        assert!(!s.contains("key_SECRET789"), "header value must be redacted: {s}");
        assert!(s.contains("https://mcp.example.com"), "url should stay visible: {s}");
        assert!(s.contains("X-Api-Key"), "header key should stay visible: {s}");
    }

    /// cas-9f07: with `--show-secrets`, `redact_server_secrets` is not called, so
    /// the serialized value retains the plaintext secret.
    #[test]
    fn show_secrets_path_keeps_plaintext() {
        let cfg = ServerConfig::Stdio {
            command: "cas".into(),
            args: vec![],
            env: HashMap::from([("TOKEN".to_string(), "plain_SECRET".to_string())]),
        };
        let v = serde_json::to_value(&cfg).unwrap();
        let s = serde_json::to_string(&v).unwrap();
        assert!(s.contains("plain_SECRET"), "un-redacted value retains secret: {s}");
    }
}
