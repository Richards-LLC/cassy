//! Pre-trust a working directory for the Codex CLI (cas-28a49, GH #97).
//!
//! # Why this exists
//!
//! Codex CLI refuses to start in a directory that is absent from the
//! `[projects]` trust table in `$CODEX_HOME/config.toml` (default
//! `~/.codex/config.toml`). It parks on its interactive "do you trust the files
//! in this folder?" onboarding screen **before** it renders a TUI, before it
//! writes a session file, and before it starts any MCP server — so a factory
//! worker launched there never registers with CAS and the spawn dies at
//! `stage=register` with a generic 60s timeout.
//!
//! Verified against codex-cli 0.146.0 on Linux by launching `codex --yolo
//! --no-alt-screen` under a real PTY in three configurations:
//!
//! | workdir state | bytes of TUI output in 12s |
//! |---|---|
//! | listed in `[projects]` as `trusted` | ~27 000 (normal startup) |
//! | absent from `[projects]` | 115 (terminal capability queries only — wedged) |
//! | absent, launched with `-c 'projects."<dir>".trust_level="trusted"'` | 115 (wedged) |
//!
//! The third row is the important one: the CLI `-c` override does **not**
//! satisfy the trust check, so the entry has to exist on disk before launch.
//! That is what [`ensure_project_trusted`] does.
//!
//! The write is deliberately conservative — corrupting this file would stop
//! Codex from starting *anywhere*, which is strictly worse than the wedge being
//! fixed:
//! - it only ever **appends** a `[projects."<abs path>"]` block; it never
//!   rewrites, reformats, or reorders the operator's config;
//! - presence is decided by parsing the TOML, not by matching lines, so an
//!   existing entry in any spelling (`[projects."x"] # comment`, an inline
//!   `[projects]` entry, a dotted `projects."x".trust_level = …`) is left
//!   completely alone, whatever its `trust_level` — an operator who explicitly
//!   untrusted a directory keeps that decision, and no duplicate table is ever
//!   created;
//! - a config that does not parse, a result that would not parse, and a path
//!   that cannot be a TOML key (control characters, non-UTF-8) all abort the
//!   write instead of guessing;
//! - the read-modify-write/read-back transaction is guarded by a blocking OS
//!   advisory lock so a concurrent factory process (or another CAS-owned
//!   Codex registration) cannot lose an update or launch before its state is
//!   verified;
//! - the file is replaced via write-temp-then-rename, through any symlink and
//!   preserving the original mode, then the file and its parent directory are
//!   synced and parsed back before launch; a concurrently reading Codex process
//!   never observes a half-written config and a `0600` config stays `0600`.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use fs2::FileExt;
use sha2::{Digest, Sha256};

/// Native Codex uses seconds. Four times the incident's ~30s hook delay;
/// Cargo policy validation also has its own much smaller response deadline.
pub const CODEX_PRE_TOOL_TIMEOUT_SECS: u64 = 120;

/// What [`ensure_project_trusted`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexTrustOutcome {
    /// Every candidate path already had a `[projects."..."]` entry.
    AlreadyPresent,
    /// One or more trust entries were appended to the config file.
    Added(Vec<String>),
    /// Nothing was attempted; the reason is operator-facing.
    Skipped(&'static str),
}

/// Resolve the directory Codex reads its `config.toml` from.
///
/// `$CODEX_HOME` wins when set (that is Codex's own override), otherwise
/// `~/.codex`.
pub fn codex_home() -> Option<PathBuf> {
    match std::env::var_os("CODEX_HOME") {
        Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => dirs::home_dir().map(|home| home.join(".codex")),
    }
}

/// Escape a path for use inside a TOML basic string key.
fn escape_toml_basic(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(ch),
        }
    }
    out
}

/// The CAS-native Codex handlers whose hash state CAS owns. Codex's trust
/// key also includes a positional matcher-group and handler index, so the
/// values are discovered from the generated `hooks.json` before writing state.
///
/// The tool guards are required in every file CAS generates. The lifecycle
/// handlers (cas-d0fe) are written only to the user-level `hooks.json`, so a
/// project file without them is still a complete CAS file.
const CAS_CODEX_HOOKS: [CasCodexHook; 4] = [
    CasCodexHook {
        event: "PreToolUse",
        event_key: "pre_tool_use",
        command: "CAS_HOOK_HARNESS=codex cas hook PreToolUse",
        required: true,
    },
    CasCodexHook {
        event: "PostToolUse",
        event_key: "post_tool_use",
        command: "cas hook PostToolUse",
        required: true,
    },
    CasCodexHook {
        event: "SessionStart",
        event_key: "session_start",
        command: "CAS_HOOK_HARNESS=codex cas hook SessionStart",
        required: false,
    },
    CasCodexHook {
        event: "Stop",
        event_key: "stop",
        command: "CAS_HOOK_HARNESS=codex cas hook Stop",
        required: false,
    },
];

/// One CAS-owned Codex hook handler.
#[derive(Debug, Clone, Copy)]
struct CasCodexHook {
    event: &'static str,
    event_key: &'static str,
    command: &'static str,
    required: bool,
}

/// Codex events whose `matcher` is ignored. Codex 0.163 `hooks/list` reports
/// them with `matcher: null` and leaves the key out of their trust identity,
/// even when `hooks.json` spells `"matcher": ""` (cas-d0fe).
fn event_ignores_matcher(event_key: &str) -> bool {
    matches!(event_key, "stop" | "user_prompt_submit")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HookTrustEntry {
    key: String,
    hash: String,
}

/// Return the canonical Codex trust hash for a normalized command hook.
///
/// Captured against Codex CLI 0.147.0 using its `hooks/list` RPC and confirmed
/// against the upstream `NormalizedHookIdentity` implementation: SHA-256 of
/// canonical JSON for `{ event_name, matcher, hooks: [handler] }`. Events that
/// ignore matchers omit the `matcher` key (Codex 0.163 `hooks/list`, cas-d0fe).
fn codex_command_hook_hash(
    event_name: &str,
    matcher: &str,
    command: &str,
    timeout: u64,
    runs_async: bool,
) -> String {
    let mut identity = serde_json::json!({
        "event_name": event_name,
        "hooks": [{
            "type": "command",
            "command": command,
            "timeout": timeout,
            "async": runs_async,
        }],
    });
    if !event_ignores_matcher(event_name) {
        identity["matcher"] = matcher.into();
    }
    // `serde_json::Map` is a BTreeMap in our configuration, so this emits the
    // recursively sorted canonical form Codex hashes. The regression below pins
    // the two hashes returned by the real CLI.
    let bytes = serde_json::to_vec(&identity).expect("JSON hook identity serializes");
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn cas_hook_trust_entries(hooks_path: &Path) -> io::Result<Vec<HookTrustEntry>> {
    if !hooks_path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Codex hooks path must be absolute to register trust",
        ));
    }
    let source = hooks_path.to_string_lossy().to_string();
    if path_is_unsafe_key(&source) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Codex hooks path contains control characters or invalid UTF-8",
        ));
    }
    let contents = std::fs::read_to_string(hooks_path)?;
    let hooks: serde_json::Value = serde_json::from_str(&contents).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("failed to parse {}: {error}", visible_hook_path(hooks_path)),
        )
    })?;
    let root = hooks
        .get("hooks")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} has no hooks object", visible_hook_path(hooks_path)),
            )
        })?;

    let mut entries = Vec::with_capacity(CAS_CODEX_HOOKS.len());
    for CasCodexHook {
        event,
        event_key,
        command: expected_command,
        required,
    } in CAS_CODEX_HOOKS
    {
        let Some(groups) = root.get(event).and_then(serde_json::Value::as_array) else {
            if !required {
                continue;
            }
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{} is missing CAS {event} hook groups",
                    visible_hook_path(hooks_path)
                ),
            ));
        };
        let mut found = None;
        for (group_index, group) in groups.iter().enumerate() {
            let matcher = group
                .get("matcher")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let Some(handlers) = group.get("hooks").and_then(serde_json::Value::as_array) else {
                continue;
            };
            for (handler_index, handler) in handlers.iter().enumerate() {
                if handler.get("type").and_then(serde_json::Value::as_str) != Some("command")
                    || handler.get("command").and_then(serde_json::Value::as_str)
                        != Some(expected_command)
                {
                    continue;
                }
                let timeout = handler
                    .get("timeout")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(600)
                    .max(1);
                let runs_async = handler
                    .get("async")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                found = Some(HookTrustEntry {
                    key: format!("{source}:{event_key}:{group_index}:{handler_index}"),
                    hash: codex_command_hook_hash(
                        event_key,
                        matcher,
                        expected_command,
                        timeout,
                        runs_async,
                    ),
                });
                break;
            }
            if found.is_some() {
                break;
            }
        }
        match found {
            Some(entry) => entries.push(entry),
            None if !required => {}
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "{} does not contain canonical CAS {event} command",
                        visible_hook_path(hooks_path)
                    ),
                ));
            }
        }
    }
    Ok(entries)
}

fn hook_state_is_trusted(config: &str, entry: &HookTrustEntry) -> Result<bool, toml::de::Error> {
    let parsed: toml::Value = toml::from_str(config)?;
    Ok(parsed
        .get("hooks")
        .and_then(|hooks| hooks.get("state"))
        .and_then(|state| state.get(&entry.key))
        .and_then(|state| state.get("trusted_hash"))
        .and_then(toml::Value::as_str)
        == Some(entry.hash.as_str()))
}

fn hook_state_entry_block(entry: &HookTrustEntry) -> String {
    format!(
        "\n[hooks.state.\"{}\"]\ntrusted_hash = \"{}\"\n",
        escape_toml_basic(&entry.key),
        entry.hash
    )
}

/// Update the exact table spelling CAS itself appends, retaining all unrelated
/// comments and configuration bytes. Other valid TOML spellings are deliberately
/// not rewritten: CAS must never reserialize an operator's whole Codex config
/// merely to refresh a hook hash.
fn refresh_owned_hook_state(existing: &str, entry: &HookTrustEntry) -> io::Result<Option<String>> {
    if hook_state_is_trusted(existing, entry)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?
    {
        return Ok(None);
    }
    let parsed: toml::Value = toml::from_str(existing)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    let has_state = parsed
        .get("hooks")
        .and_then(|hooks| hooks.get("state"))
        .and_then(|state| state.get(&entry.key))
        .is_some();
    if !has_state {
        let mut updated = existing.to_string();
        if !updated.is_empty() && !updated.ends_with('\n') {
            updated.push('\n');
        }
        updated.push_str(&hook_state_entry_block(entry));
        return Ok(Some(updated));
    }

    let header = format!("[hooks.state.\"{}\"]", escape_toml_basic(&entry.key));
    let Some(header_start) = existing
        .lines()
        .scan(0usize, |offset, line| {
            let start = *offset;
            *offset += line.len() + 1;
            Some((start, line))
        })
        .find_map(|(start, line)| (line.trim() == header).then_some(start))
    else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "existing CAS hook trust uses an unsupported TOML spelling; refusing to rewrite it",
        ));
    };
    let after_header = existing[header_start..]
        .find('\n')
        .map(|relative| header_start + relative + 1)
        .unwrap_or(existing.len());
    let section_end = existing[after_header..]
        .find("\n[")
        .map(|relative| after_header + relative + 1)
        .unwrap_or(existing.len());
    let section = &existing[after_header..section_end];
    let Some((line_start, line_end)) = section
        .lines()
        .scan(0usize, |offset, line| {
            let start = *offset;
            *offset += line.len() + 1;
            Some((start, line))
        })
        .find_map(|(start, line)| {
            line.trim_start()
                .starts_with("trusted_hash")
                .then_some((after_header + start, after_header + start + line.len()))
        })
    else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "existing CAS hook trust lacks trusted_hash; refusing to rewrite it",
        ));
    };
    let indent = existing[line_start..line_end]
        .chars()
        .take_while(|ch| ch.is_whitespace())
        .collect::<String>();
    let mut updated = String::with_capacity(existing.len() + entry.hash.len());
    updated.push_str(&existing[..line_start]);
    updated.push_str(&format!("{indent}trusted_hash = \"{}\"", entry.hash));
    updated.push_str(&existing[line_end..]);
    Ok(Some(updated))
}

fn cas_pre_tool_handlers(
    value: &mut serde_json::Value,
) -> impl Iterator<Item = &mut serde_json::Value> {
    value
        .pointer_mut("/hooks/PreToolUse")
        .and_then(serde_json::Value::as_array_mut)
        .into_iter()
        .flatten()
        .filter_map(|group| {
            group
                .get_mut("hooks")
                .and_then(serde_json::Value::as_array_mut)
        })
        .flatten()
        .filter(|handler| {
            handler.get("command").and_then(serde_json::Value::as_str)
                == Some(CAS_CODEX_HOOKS[0].command)
        })
}

/// Parse a hooks file this module read, naming it on failure (cas-10af): a
/// bare serde error says only "EOF while parsing a value at line 1 column 0".
fn parse_hooks_json(path: &Path, source: &str) -> io::Result<serde_json::Value> {
    if source.trim().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "codex_hooks_json_missing: {} is missing or empty; Codex would launch this \
                 worker without CAS hooks, so restore it before spawning",
                visible_hook_path(path)
            ),
        ));
    }
    serde_json::from_str(source).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "codex_hooks_json_invalid: {} does not parse as JSON ({error})",
                visible_hook_path(path)
            ),
        )
    })
}

fn ensure_cas_pre_tool_budget(path: &Path) -> io::Result<bool> {
    update_codex_file_locked(
        path,
        |existing| {
            let mut value = parse_hooks_json(path, existing)?;
            let mut changed = false;
            for handler in cas_pre_tool_handlers(&mut value) {
                // An omitted timeout already inherits Codex's 600-second default.
                if handler
                    .get("timeout")
                    .and_then(serde_json::Value::as_u64)
                    .is_some_and(|seconds| seconds < CODEX_PRE_TOOL_TIMEOUT_SECS)
                {
                    handler["timeout"] = CODEX_PRE_TOOL_TIMEOUT_SECS.into();
                    changed = true;
                }
            }
            Ok(changed.then(|| serde_json::to_string_pretty(&value).unwrap() + "\n"))
        },
        |read_back| {
            let mut value = parse_hooks_json(path, read_back)?;
            if cas_pre_tool_handlers(&mut value).any(|handler| {
                handler
                    .get("timeout")
                    .and_then(serde_json::Value::as_u64)
                    .is_some_and(|seconds| seconds < CODEX_PRE_TOOL_TIMEOUT_SECS)
            }) {
                return Err(io::Error::other("CAS PreToolUse timeout read-back failed"));
            }
            Ok(())
        },
        |updated| parse_hooks_json(path, updated).map(|_| ()),
    )
}

/// Upgrade/trust only existing CAS-owned handlers, preserving other hooks and
/// the selected account home. Called for worker user and project config layers.
pub fn prepare_cas_tool_hooks_in(config: &Path, hooks: &Path) -> io::Result<bool> {
    prepare_cas_tool_hooks(config, hooks, true)
}

/// `upgrade_budget=false` trusts the file as it stands and never writes it.
fn prepare_cas_tool_hooks(config: &Path, hooks: &Path, upgrade_budget: bool) -> io::Result<bool> {
    if !has_cas_tool_hooks(hooks)? {
        return Ok(false);
    }
    if upgrade_budget {
        ensure_cas_pre_tool_budget(hooks)?;
    }
    register_cas_hook_trust(config, hooks)
}

/// Whether `hooks` exists and carries the CAS PreToolUse handler; a file that
/// does not parse is a named failure naming its path.
fn has_cas_tool_hooks(hooks: &Path) -> io::Result<bool> {
    let source = match std::fs::read_to_string(hooks) {
        Ok(source) => source,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let mut value: serde_json::Value = serde_json::from_str(&source).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "codex_hooks_json_invalid: {} does not parse as JSON ({error}); Codex would load \
                 it for this worker, so fix or remove it before spawning",
                visible_hook_path(hooks)
            ),
        )
    })?;
    Ok(cas_pre_tool_handlers(&mut value).next().is_some())
}

/// Keep controls and Unicode 17 Default_Ignorable_Code_Point visible without
/// changing the path used for filesystem operations.
fn visible_hook_path(path: &Path) -> String {
    path.display()
        .to_string()
        .chars()
        .map(|character| {
            if character.is_control()
                || matches!(character,
                    '\u{ad}' | '\u{34f}' | '\u{61c}' | '\u{115f}'..='\u{1160}'
                    | '\u{17b4}'..='\u{17b5}' | '\u{180b}'..='\u{180f}'
                    | '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}'
                    | '\u{2060}'..='\u{206f}' | '\u{3164}' | '\u{fe00}'..='\u{fe0f}'
                    | '\u{feff}' | '\u{ffa0}' | '\u{fff0}'..='\u{fff8}'
                    | '\u{1bca0}'..='\u{1bca3}' | '\u{1d173}'..='\u{1d17a}'
                    | '\u{e0000}'..='\u{e0fff}')
            {
                character.escape_default().to_string()
            } else {
                character.to_string()
            }
        })
        .collect()
}

/// Refuse a main checkout whose CAS hook bodies differ from the worker's own
/// generated copy (cas-6591). Codex 0.162 runs the main checkout's file, so a
/// stale body (observed: PreToolUse `^Bash$`, timeout 3) would be trusted and
/// silently guard less than the current hooks. Neither trust nor rewrite it.
fn ensure_main_hooks_current(main: &Path, main_hooks: &Path, own: &Path) -> io::Result<()> {
    if !has_cas_tool_hooks(own)? {
        return Ok(());
    }
    let hashes = |path: &Path| -> io::Result<Vec<String>> {
        Ok(cas_hook_trust_entries(path)
            .map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("codex_hooks_invalid: {}: {error}", visible_hook_path(path)),
                )
            })?
            .into_iter()
            .map(|entry| entry.hash)
            .collect())
    };
    // A main checkout without CAS handlers has no CAS body to be stale; the
    // user-level CAS hooks still guard the worker.
    if !has_cas_tool_hooks(main_hooks)? {
        return Ok(());
    }
    if hashes(main_hooks)? == hashes(own)? {
        return Ok(());
    }
    // Ordinary single quotes preserve spaces, history tokens and substitutions.
    // cas-pty cannot depend on the CLI's Git remedy renderer.
    let main_text = main.display().to_string();
    let advice = if visible_hook_path(main) != main_text {
        // Escaping inside ordinary shell quotes would change the target path.
        "Update the main checkout by hand, then respawn; its path contains control or invisible Unicode characters, so no copyable command is available.".to_string()
    } else {
        let main_arg = format!("'{}'", main_text.replace('\'', "'\\''"));
        format!(
            "Update the main checkout (`git -C {main_arg} pull`, or `git -C {main_arg} \
             checkout .codex/hooks.json`), then respawn."
        )
    };
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "codex_main_checkout_hooks_stale: {} carries CAS hooks that differ from {}, and \
             Codex loads the main checkout's file for a linked worktree, so the worker would run \
             stale guards. {advice}",
            visible_hook_path(main_hooks),
            visible_hook_path(own),
        ),
    ))
}

/// Trust every project `hooks.json` Codex may load for a session in `cwd`.
///
/// Codex 0.156 and earlier read `<cwd>/.codex/hooks.json`. Codex 0.162 instead
/// resolves a linked git worktree to its main checkout and loads
/// `<main checkout>/.codex/hooks.json`, keyed by that path (cas-4984). Observed
/// with `codex app-server` `hooks/list` on codex-cli 0.162.1: for a linked
/// worktree cwd it listed only the main checkout's file, with the same hashes
/// CAS computes. Both are trusted; the state entry Codex ignores is inert.
///
/// The main checkout lies outside the worker's worktree and its hooks file is
/// usually tracked, so it is never rewritten (cas-6591): it is trusted as it
/// stands only when its CAS hook bodies equal the worker's own copy, and a
/// stale one fails the spawn. Only the worktree copy gets the budget upgrade.
pub fn prepare_codex_project_hooks_in(config: &Path, cwd: &Path) -> io::Result<()> {
    let own = cwd.join(".codex/hooks.json");
    // Compare the generated copies before upgrading the worker's local budget.
    if let Some(main) = linked_worktree_main_checkout(cwd) {
        let main_hooks = main.join(".codex/hooks.json");
        if main_hooks != own {
            ensure_main_hooks_current(&main, &main_hooks, &own)?;
            prepare_cas_tool_hooks(config, &main_hooks, false)?;
        }
    }
    prepare_cas_tool_hooks(config, &own, true)?;
    Ok(())
}

/// How Codex will judge one command hook handler at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexHookTrust {
    /// The stored hash equals the handler's current identity; it runs.
    Trusted,
    /// No stored hash; Codex skips it or parks on "Hooks need review".
    Untrusted,
    /// A stored hash for a different definition; treated like untrusted.
    Modified,
    /// Explicitly disabled in config; it never runs and never prompts.
    Disabled,
}

/// One handler from a `hooks.json` Codex may load for the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexHookReadiness {
    /// Codex's trust key: `<source>:<event>:<group>:<handler>`.
    pub key: String,
    pub command: String,
    /// Whether this is one of CAS's own handlers (exact command match).
    pub cas_owned: bool,
    pub trust: CodexHookTrust,
}

/// Read-only verdict on whether a Codex launch will start without parking on
/// a trust prompt (cas-d0fe). Only CAS's own handlers and the directory trust
/// are launch requirements; any other hook is reported, never trusted by CAS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexLaunchReadiness {
    pub cwd: PathBuf,
    pub project_trusted: bool,
    pub hooks: Vec<CodexHookReadiness>,
}

impl CodexLaunchReadiness {
    /// Reasons this launch would park: an untrusted directory, or a CAS
    /// handler that is untrusted or modified. A disabled CAS handler is an
    /// operator choice and does not prompt, so it is not a reason.
    pub fn blocking(&self) -> Vec<String> {
        let mut reasons = Vec::new();
        if !self.project_trusted {
            reasons.push(format!(
                "directory {} is not trusted in Codex [projects]",
                self.cwd.display()
            ));
        }
        for hook in self.hooks.iter().filter(|hook| hook.cas_owned) {
            if matches!(
                hook.trust,
                CodexHookTrust::Untrusted | CodexHookTrust::Modified
            ) {
                reasons.push(format!("CAS hook {} is {:?}", hook.key, hook.trust));
            }
        }
        reasons
    }

    /// Enabled hooks CAS does not own whose definitions are not trusted. CAS
    /// never trusts these; the operator reviews them in Codex `/hooks`.
    pub fn foreign_needing_review(&self) -> Vec<&CodexHookReadiness> {
        self.hooks
            .iter()
            .filter(|hook| {
                !hook.cas_owned
                    && matches!(
                        hook.trust,
                        CodexHookTrust::Untrusted | CodexHookTrust::Modified
                    )
            })
            .collect()
    }
}

/// `PreToolUse` → `pre_tool_use`, the event spelling Codex uses in trust keys.
fn codex_event_key(event: &str) -> String {
    let mut key = String::with_capacity(event.len() + 4);
    for (index, ch) in event.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if index > 0 {
                key.push('_');
            }
            key.push(ch.to_ascii_lowercase());
        } else {
            key.push(ch);
        }
    }
    key
}

/// Every command handler in `hooks_path`, judged against `config`'s state.
fn hook_file_readiness(
    config: &toml::Value,
    hooks_path: &Path,
) -> io::Result<Vec<CodexHookReadiness>> {
    let contents = match std::fs::read_to_string(hooks_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let hooks: serde_json::Value = serde_json::from_str(&contents).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("failed to parse {}: {error}", hooks_path.display()),
        )
    })?;
    let state = config.get("hooks").and_then(|hooks| hooks.get("state"));
    let mut out = Vec::new();
    let Some(events) = hooks.get("hooks").and_then(serde_json::Value::as_object) else {
        return Ok(out);
    };
    for (event, groups) in events {
        let event_key = codex_event_key(event);
        for (group_index, group) in groups.as_array().into_iter().flatten().enumerate() {
            let matcher = group
                .get("matcher")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let handlers = group.get("hooks").and_then(serde_json::Value::as_array);
            for (handler_index, handler) in handlers.into_iter().flatten().enumerate() {
                if handler.get("type").and_then(serde_json::Value::as_str) != Some("command") {
                    continue;
                }
                let Some(command) = handler.get("command").and_then(serde_json::Value::as_str)
                else {
                    continue;
                };
                let timeout = handler
                    .get("timeout")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(600)
                    .max(1);
                let runs_async = handler
                    .get("async")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                let key = format!(
                    "{}:{event_key}:{group_index}:{handler_index}",
                    hooks_path.display()
                );
                let hash =
                    codex_command_hook_hash(&event_key, matcher, command, timeout, runs_async);
                let entry = state.and_then(|state| state.get(&key));
                let trust = if entry
                    .and_then(|entry| entry.get("enabled"))
                    .and_then(toml::Value::as_bool)
                    == Some(false)
                {
                    CodexHookTrust::Disabled
                } else {
                    match entry
                        .and_then(|entry| entry.get("trusted_hash"))
                        .and_then(toml::Value::as_str)
                    {
                        Some(stored) if stored == hash => CodexHookTrust::Trusted,
                        Some(_) => CodexHookTrust::Modified,
                        None => CodexHookTrust::Untrusted,
                    }
                };
                out.push(CodexHookReadiness {
                    key,
                    command: command.to_string(),
                    cas_owned: CAS_CODEX_HOOKS.iter().any(|hook| hook.command == command),
                    trust,
                });
            }
        }
    }
    Ok(out)
}

/// Judge, without writing anything, the trust state Codex will see for a
/// session in `cwd` under `codex_home`: directory trust plus every handler in
/// the user `hooks.json`, `<cwd>/.codex/hooks.json`, and the main checkout's
/// `.codex/hooks.json` for a linked worktree (the file Codex 0.162+ loads).
pub fn codex_launch_readiness(
    config_path: &Path,
    codex_home: &Path,
    cwd: &Path,
) -> io::Result<CodexLaunchReadiness> {
    let contents = match std::fs::read_to_string(config_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    let config: toml::Value = toml::from_str(&contents)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    let project_trusted = candidate_keys(cwd).iter().any(|key| {
        config
            .get("projects")
            .and_then(|projects| projects.get(key))
            .and_then(|project| project.get("trust_level"))
            .and_then(toml::Value::as_str)
            == Some("trusted")
    });
    let mut files = vec![codex_home.join("hooks.json"), cwd.join(".codex/hooks.json")];
    if let Some(main) = linked_worktree_main_checkout(cwd) {
        files.push(main.join(".codex/hooks.json"));
    }
    files.dedup();
    let mut hooks = Vec::new();
    for file in &files {
        hooks.extend(hook_file_readiness(&config, file)?);
    }
    Ok(CodexLaunchReadiness {
        cwd: cwd.to_path_buf(),
        project_trusted,
        hooks,
    })
}

/// The main checkout of the linked git worktree containing `cwd`, if any.
///
/// A linked worktree's `.git` is a file `gitdir: <repo>/.git/worktrees/<name>`
/// whose `commondir` names the shared `.git`; the main checkout is its parent.
/// Read from the files directly so trust preparation needs no `git` binary.
fn linked_worktree_main_checkout(cwd: &Path) -> Option<PathBuf> {
    let dot_git = cwd
        .ancestors()
        .map(|dir| dir.join(".git"))
        .find(|p| p.exists())?;
    if dot_git.is_dir() {
        return None;
    }
    let pointer = std::fs::read_to_string(&dot_git).ok()?;
    let gitdir = Path::new(pointer.trim().strip_prefix("gitdir:")?.trim());
    let gitdir = dot_git.parent()?.join(gitdir);
    let commondir = std::fs::read_to_string(gitdir.join("commondir")).ok()?;
    let common = normalize_lexically(&gitdir.join(commondir.trim()));
    (common.file_name()? == ".git").then(|| common.parent().map(Path::to_path_buf))?
}

fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Record the current CAS hook definitions as trusted in Codex's user config.
///
/// Codex uses this separate state from project trust: without it the project
/// layer loads, but non-managed handlers are silently skipped until someone
/// opens `/hooks`. Uses the same lock/fsync/read-back transaction as the
/// project-trust writer, and only appends CAS-owned state entries. A changed
/// definition deliberately receives a new state key only after the generator
/// has emitted its canonical handler shape.
pub fn ensure_cas_hooks_trusted_in(config_path: &Path, hooks_path: &Path) -> io::Result<bool> {
    ensure_cas_pre_tool_budget(hooks_path)?;
    register_cas_hook_trust(config_path, hooks_path)
}

/// Record the CAS handlers in `hooks_path` as trusted, hashing the file as it
/// stands; never writes the hooks file.
fn register_cas_hook_trust(config_path: &Path, hooks_path: &Path) -> io::Result<bool> {
    let entries = cas_hook_trust_entries(hooks_path)?;
    let entries_for_write = entries.clone();
    update_codex_config_locked(
        config_path,
        move |existing| {
            let mut updated = existing.to_string();
            let mut changed = false;
            for entry in &entries_for_write {
                if let Some(next) = refresh_owned_hook_state(&updated, entry)? {
                    updated = next;
                    changed = true;
                }
            }
            Ok(changed.then_some(updated))
        },
        |read_back| {
            for entry in &entries {
                if !hook_state_is_trusted(read_back, entry).map_err(|error| {
                    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
                })? {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "read-back verification failed for CAS hook state {}",
                            entry.key
                        ),
                    ));
                }
            }
            Ok(())
        },
    )
}

/// Resolve the active Codex user config (`CODEX_HOME` or `~/.codex`) and trust
/// the CAS handlers in `hooks_path` without an interactive `/hooks` review.
pub fn ensure_cas_hooks_trusted(hooks_path: &Path) -> io::Result<bool> {
    let home = codex_home().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "could not resolve CODEX_HOME or a home directory for Codex hook trust",
        )
    })?;
    ensure_cas_hooks_trusted_in(&home.join("config.toml"), hooks_path)
}

/// The literal block appended for a newly trusted path.
pub fn trust_entry_block(path: &str) -> String {
    format!(
        "\n[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
        escape_toml_basic(path)
    )
}

/// Does `config` already declare a project entry for `path`?
///
/// Parsed with a real TOML parser rather than matched line-by-line, because
/// `[projects."x"]`, `[projects."x"] # comment`, `[ projects."x" ]`,
/// `projects."x".trust_level = ...` and an inline `[projects]` table entry are
/// all the same key. Missing any of those spellings and appending a second
/// `[projects."x"]` table would make the whole file invalid TOML — TOML forbids
/// redeclaring a table — which would stop Codex from starting **anywhere**, a
/// far worse failure than the bug being fixed.
///
/// `Err` means the config does not parse at all; the caller must then refuse to
/// touch it.
///
/// Only the key's presence is checked, never `trust_level`'s value: an existing
/// entry means the operator (or Codex itself) already decided about this
/// directory.
pub fn config_has_project(config: &str, path: &str) -> Result<bool, toml::de::Error> {
    let parsed: toml::Value = toml::from_str(config)?;
    Ok(parsed
        .get("projects")
        .and_then(|projects| projects.get(path))
        .is_some())
}

/// Paths that cannot be written as a TOML basic-string key without corrupting
/// (or, with an embedded newline, *injecting into*) the operator's config.
fn path_is_unsafe_key(path: &str) -> bool {
    path.contains('\u{FFFD}') || path.chars().any(|c| c.is_control())
}

/// Path keys to trust for `workdir`: the path as spawned, plus its canonical
/// form when symlink resolution changes it (Codex matches on the cwd string it
/// resolves, and a factory worktree can be reached through either).
fn candidate_keys(workdir: &Path) -> Vec<String> {
    let mut keys = vec![workdir.to_string_lossy().to_string()];
    if let Ok(canonical) = std::fs::canonicalize(workdir) {
        let canonical = canonical.to_string_lossy().to_string();
        if !keys.contains(&canonical) {
            keys.push(canonical);
        }
    }
    keys
}

/// Serializes config mutations within this process. Across processes,
/// [`ConfigLock`] guards the same transaction.
fn config_write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Cross-process lock over the read-modify-write/read-back window.
///
/// Atomic rename only prevents a *torn read*; it does not prevent a lost
/// update. Two factory processes (or a human answering Codex's own trust prompt
/// in the window) would otherwise have the later rename revert the whole file to
/// the earlier snapshot. This deliberately blocks instead of falling back to an
/// unlocked write: spawning Codex before its entry is durable reintroduces the
/// trust-prompt race this lock exists to prevent. OS advisory locks release when
/// a process exits, including a crash, so there is no stale-lock timeout path.
struct ConfigLock {
    file: File,
}

impl ConfigLock {
    fn acquire(config_path: &Path) -> io::Result<Self> {
        let path = config_path.with_extension("toml.cas-lock");
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)?;
        file.lock_exclusive()?;
        Ok(Self { file })
    }
}

impl Drop for ConfigLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn verify_trusted_project_entry(contents: &str, keys: &[String]) -> io::Result<()> {
    let parsed: toml::Value = toml::from_str(contents).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("read-back TOML parse failed while verifying [projects]: {error}"),
        )
    })?;
    let mut trusted = false;
    for key in keys {
        trusted |= parsed
            .get("projects")
            .and_then(|projects| projects.get(key))
            .and_then(|project| project.get("trust_level"))
            .and_then(toml::Value::as_str)
            == Some("trusted");
    }
    if !trusted {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("read-back verification failed: no trusted [projects.{keys:?}] entry"),
        ));
    }
    Ok(())
}

/// Persist the directory entry created by the rename as well as the file's
/// contents. This is a no-op on platforms where opening a directory for fsync
/// is not supported; the read-back still guards the launch ordering there.
#[cfg(unix)]
fn sync_parent_directory(config_path: &Path) -> io::Result<()> {
    if let Some(parent) = config_path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn sync_parent_directory(_config_path: &Path) -> io::Result<()> {
    Ok(())
}

/// Mutate Codex's user config while holding the single transaction lock used by
/// every CAS-owned Codex registration.
///
/// `mutate` receives the complete current TOML and returns `Some(updated)` only
/// when it needs to replace the file. `verify` receives a freshly read file
/// *after* the temp file and containing directory are synced, while the lock is
/// still held. It must reject any state that would make a caller unsafe to
/// launch Codex. This lets project trust and hook approval state share one
/// read-modify-write/read-back boundary without either feature having to parse
/// or serialize the other's tables.
pub fn update_codex_config_locked<F, V>(
    config_path: &Path,
    mutate: F,
    verify: V,
) -> io::Result<bool>
where
    F: FnOnce(&str) -> io::Result<Option<String>>,
    V: FnOnce(&str) -> io::Result<()>,
{
    update_codex_file_locked(config_path, mutate, verify, |updated| {
        toml::from_str::<toml::Value>(updated)
            .map(|_| ())
            .map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: {error}", config_path.display()),
                )
            })
    })
}

fn update_codex_file_locked<F, V, P>(
    config_path: &Path,
    mutate: F,
    verify: V,
    parse: P,
) -> io::Result<bool>
where
    F: FnOnce(&str) -> io::Result<Option<String>>,
    V: FnOnce(&str) -> io::Result<()>,
    P: FnOnce(&str) -> io::Result<()>,
{
    let _guard = config_write_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // `read_to_string` follows a symlink but `rename` would replace the link.
    // Resolve first so managed writes preserve a dotfiles-managed link.
    let config_path = std::fs::canonicalize(config_path)
        .ok()
        .unwrap_or_else(|| config_path.to_path_buf());
    let _lock = ConfigLock::acquire(&config_path)?;
    let named = |error: io::Error| {
        io::Error::new(error.kind(), format!("{}: {error}", config_path.display()))
    };
    let existing = match std::fs::read_to_string(&config_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(named(error)),
    };

    let changed = if let Some(updated) = mutate(&existing)? {
        parse(&updated)?;
        let tmp = config_path.with_extension(format!("toml.cas-{}", std::process::id()));
        std::fs::write(&tmp, &updated)?;
        // Rename replaces original metadata; preserve a restrictive config mode.
        if let Ok(meta) = std::fs::metadata(&config_path) {
            let _ = std::fs::set_permissions(&tmp, meta.permissions());
        }
        OpenOptions::new().read(true).open(&tmp)?.sync_all()?;
        if let Err(error) = std::fs::rename(&tmp, &config_path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(error);
        }
        sync_parent_directory(&config_path)?;
        true
    } else {
        false
    };

    let read_back = std::fs::read_to_string(&config_path).map_err(named)?;
    verify(&read_back)?;
    Ok(changed)
}

/// Ensure `workdir` is trusted in the Codex config at `config_path`.
///
/// Creates the file (and its parent directory) when missing. Refuses to write
/// when the existing config does not parse, when the result would not parse, or
/// when the path cannot be represented as a TOML key — corrupting
/// `config.toml` would break every Codex launch on the machine, which is
/// strictly worse than the wedge this function prevents.
pub fn ensure_project_trusted_in(
    config_path: &Path,
    workdir: &Path,
) -> io::Result<CodexTrustOutcome> {
    if !workdir.is_absolute() {
        return Ok(CodexTrustOutcome::Skipped(
            "worker cwd is not absolute; cannot key a Codex trust entry",
        ));
    }
    let keys = candidate_keys(workdir);
    if keys.iter().any(|key| path_is_unsafe_key(key)) {
        return Ok(CodexTrustOutcome::Skipped(
            "worker cwd contains control characters or is not valid UTF-8; refusing to write a \
             Codex trust entry for it",
        ));
    }

    let keys_for_write = keys.clone();
    let changed = match update_codex_config_locked(
        config_path,
        move |existing| {
            let mut already_present = false;
            for key in &keys_for_write {
                match config_has_project(existing, key) {
                    Ok(false) => {}
                    Ok(true) => already_present = true,
                    Err(error) => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("Codex config does not parse as TOML: {error}"),
                        ));
                    }
                }
            }
            if already_present {
                return Ok(None);
            }
            let mut updated = existing.to_string();
            if !updated.is_empty() && !updated.ends_with('\n') {
                updated.push('\n');
            }
            for key in &keys_for_write {
                updated.push_str(&trust_entry_block(key));
            }
            Ok(Some(updated))
        },
        |read_back| verify_trusted_project_entry(read_back, &keys),
    ) {
        Ok(changed) => changed,
        Err(error) if error.kind() == io::ErrorKind::InvalidData => {
            tracing::warn!(
                config = %config_path.display(),
                error = %error,
                "cas-3603: Codex config mutation/read-back verification failed; leaving launch blocked"
            );
            return Ok(CodexTrustOutcome::Skipped(
                "the Codex config could not be verified after mutation; refusing to launch",
            ));
        }
        Err(error) => return Err(error),
    };
    Ok(if changed {
        CodexTrustOutcome::Added(keys)
    } else {
        CodexTrustOutcome::AlreadyPresent
    })
}

/// Ensure `workdir` is trusted in the resolved Codex config
/// (`$CODEX_HOME/config.toml`, else `~/.codex/config.toml`).
///
/// The caller must treat any error or `Skipped` result as a failed precondition
/// and must not launch Codex. Starting first would let Codex cache a missing
/// trust entry and park at its interactive prompt even if a later write lands.
pub fn ensure_project_trusted(workdir: &Path) -> io::Result<CodexTrustOutcome> {
    let Some(home) = codex_home() else {
        return Ok(CodexTrustOutcome::Skipped(
            "could not resolve CODEX_HOME or a home directory for ~/.codex",
        ));
    };
    let config_path = home.join("config.toml");
    match ensure_project_trusted_in(&config_path, workdir) {
        Ok(CodexTrustOutcome::Added(keys)) => {
            tracing::info!(
                config = %config_path.display(),
                paths = ?keys,
                "cas-28a49: pre-trusted workdir for Codex; without this the CLI parks on its \
                 interactive trust prompt and the worker never registers"
            );
            Ok(CodexTrustOutcome::Added(keys))
        }
        Ok(other) => Ok(other),
        Err(e) => {
            tracing::warn!(
                config = %config_path.display(),
                workdir = %workdir.display(),
                error = %e,
                "cas-28a49: could not pre-trust workdir for Codex; if the worker never registers, \
                 add [projects.\"<workdir>\"] trust_level = \"trusted\" to the Codex config"
            );
            Err(e)
        }
    }
}

/// Operator-facing hint appended to a Codex worker's registration-timeout
/// diagnostic (cas-28a49, GH #97). Named separately so the daemon and its tests
/// share one string.
pub const CODEX_TRUST_TIMEOUT_HINT: &str = "This worker runs cli=codex: Codex refuses to start in a directory that is not listed in \
     [projects] in $CODEX_HOME/config.toml (default ~/.codex/config.toml), parking on its \
     interactive trust prompt before it can register. CAS pre-trusts the worker's cwd at launch, \
     so check that file for a [projects.\"<worker cwd>\"] entry with trust_level = \"trusted\"; if \
     it is missing, the daemon log records why the write was skipped (unparseable config, or the \
     entry already existed with another trust_level).";

/// Lower-case pane text of Codex's interactive hook-trust review screen
/// ("Hooks need review: N hooks are new or changed").
pub const CODEX_HOOK_REVIEW_MARKER: &str = "hooks need review";

/// Named spawn failure for a Codex pane parked on that screen (cas-4984). CAS
/// wrote hook trust before launch, so reaching it means this Codex version keys
/// or hashes hook trust in a way CAS does not recognize.
pub const CODEX_HOOK_REVIEW_FAILURE: &str = "codex_hook_trust_unrecognized: Codex parked on its \
     \"Hooks need review\" prompt although CAS pre-trusted the worker's hooks, so this Codex \
     version keys or hashes hook trust differently. Run `codex app-server` hooks/list in the \
     worker cwd and compare each hook's key and currentHash with the [hooks.state] entries in \
     $CODEX_HOME/config.toml, or pin the validated Codex version.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_existing_entry_in_every_toml_spelling() {
        // cas-28a49 review follow-up: a matcher that misses any of these would
        // append a duplicate [projects."x"] table, and TOML forbids redeclaring
        // a table — the config would stop parsing and Codex would fail to start
        // for EVERY directory, not just this one.
        for config in [
            "[projects.\"/home/a/proj\"]\ntrust_level = \"trusted\"\n",
            "[projects.'/home/a/proj']\ntrust_level = \"trusted\"\n",
            "[projects.\"/home/a/proj\"] # main repo\ntrust_level = \"trusted\"\n",
            "[ projects.\"/home/a/proj\" ]\ntrust_level = \"trusted\"\n",
            "[projects]\n\"/home/a/proj\" = { trust_level = \"trusted\" }\n",
            "projects.\"/home/a/proj\".trust_level = \"trusted\"\n",
        ] {
            assert!(
                config_has_project(config, "/home/a/proj").unwrap(),
                "must detect the existing key in: {config}"
            );
            assert!(
                !config_has_project(config, "/home/a/other").unwrap(),
                "must not report an unrelated key present in: {config}"
            );
        }
    }

    #[test]
    fn does_not_match_prefixes_or_other_tables() {
        let config = "[projects.\"/home/a/proj-two\"]\n[mcp_servers.cs]\n";
        assert!(!config_has_project(config, "/home/a/proj").unwrap());
        assert!(!config_has_project(config, "cs").unwrap());
    }

    #[test]
    fn unparseable_config_is_reported_not_guessed() {
        assert!(config_has_project("this is [not( toml", "/home/a/proj").is_err());
    }

    #[test]
    fn escaped_keys_round_trip() {
        let path = "/home/a/we\"ird\\dir";
        let block = trust_entry_block(path);
        assert!(
            config_has_project(&block, path).unwrap(),
            "block was: {block}"
        );
    }

    #[test]
    fn appends_entry_when_missing_and_creates_file() {
        let dir = TempDir::new();
        let config_path = dir.join("config.toml");
        let workdir = dir.join("proj");
        std::fs::create_dir_all(&workdir).unwrap();

        let outcome = ensure_project_trusted_in(&config_path, &workdir).unwrap();
        assert!(
            matches!(outcome, CodexTrustOutcome::Added(_)),
            "first call must append an entry, got {outcome:?}"
        );
        let written = std::fs::read_to_string(&config_path).unwrap();
        assert!(
            config_has_project(&written, &workdir.to_string_lossy()).unwrap(),
            "config must contain the trust entry: {written}"
        );
        assert!(written.contains("trust_level = \"trusted\""));
    }

    #[test]
    fn second_call_is_a_noop_and_preserves_operator_content() {
        let dir = TempDir::new();
        let config_path = dir.join("config.toml");
        let workdir = dir.join("proj");
        std::fs::create_dir_all(&workdir).unwrap();
        std::fs::write(&config_path, "# operator comment\nmodel = \"gpt-5\"\n").unwrap();

        ensure_project_trusted_in(&config_path, &workdir).unwrap();
        let after_first = std::fs::read_to_string(&config_path).unwrap();
        let outcome = ensure_project_trusted_in(&config_path, &workdir).unwrap();
        assert_eq!(outcome, CodexTrustOutcome::AlreadyPresent);
        let after_second = std::fs::read_to_string(&config_path).unwrap();
        assert_eq!(after_first, after_second, "second call must not rewrite");
        assert!(after_second.contains("# operator comment"));
        assert!(after_second.contains("model = \"gpt-5\""));
    }

    #[test]
    fn locked_transaction_supports_independent_hook_state_mutation_and_read_back() {
        // cas-3603: hooks-trust registration uses the same transaction as
        // `[projects]`; this proves an unrelated config table is retained and
        // must pass a fresh verifier before the transaction returns.
        let dir = TempDir::new();
        let config_path = dir.join("config.toml");
        std::fs::write(&config_path, "model = \"gpt-5\"\n").unwrap();
        let hook_state = "[hooks.state.\"/tmp/hooks.json:pre_tool_use:cas:handler\"]\ntrusted_hash = \"sha256:example\"\n";

        let changed = update_codex_config_locked(
            &config_path,
            |existing| Ok(Some(format!("{existing}\n{hook_state}"))),
            |read_back| {
                let parsed: toml::Value = toml::from_str(read_back).map_err(|error| {
                    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
                })?;
                let hash = parsed
                    .get("hooks")
                    .and_then(|value| value.get("state"))
                    .and_then(|value| value.get("/tmp/hooks.json:pre_tool_use:cas:handler"))
                    .and_then(|value| value.get("trusted_hash"))
                    .and_then(toml::Value::as_str);
                if hash == Some("sha256:example") {
                    Ok(())
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "hook state was not present after read-back",
                    ))
                }
            },
        )
        .unwrap();
        assert!(changed);
        assert!(
            std::fs::read_to_string(&config_path)
                .unwrap()
                .contains("model = \"gpt-5\"")
        );
    }

    #[test]
    fn codex_hook_hashes_match_the_live_cli_canonical_identities() {
        // Recorded from `codex app-server` hooks/list with codex-cli 0.147.0.
        // Codex normalizes the hook identity before hashing, so matching the
        // generated JSON bytes themselves is not sufficient.
        assert_eq!(
            codex_command_hook_hash(
                "pre_tool_use",
                "^Bash$",
                "CAS_HOOK_HARNESS=codex cas hook PreToolUse",
                3,
                false,
            ),
            "sha256:1acc1fe9f8d33d69fb3a16be30738b902dac8e20155fdb9a6490b2a52545ca77"
        );
        assert_eq!(
            codex_command_hook_hash("post_tool_use", "^Bash$", "cas hook PostToolUse", 3, false),
            "sha256:94c125201a0019ec158d7abb96cd3fc0d8282bb499aa506f202ce794748cdb2b"
        );
        // cas-d0fe: recorded from `codex app-server` hooks/list with codex-cli
        // 0.163.0. Matcher-ignored events report `matcher: null` and hash
        // without the key, even though hooks.json spelled `"matcher": ""`.
        let canary = "/usr/bin/python3 -I /mnt/rewind/cas/scratch/d0fe-canary.V68n7o/bin/hook.py";
        assert_eq!(
            codex_command_hook_hash("stop", "", &format!("{canary} Stop"), 30, false),
            "sha256:bff64b2c90c0cb52b8a199adffcc99cf6fe22551237a209f8a9878d1330d42ea"
        );
        assert_eq!(
            codex_command_hook_hash(
                "user_prompt_submit",
                "",
                &format!("{canary} UserPromptSubmit"),
                30,
                false
            ),
            "sha256:b40740aab62ccfbbeee73a15cec823c1a1b2f46862ebfc4f2ad742fac8791f2d"
        );
    }

    fn readiness_fixture(dir: &TempDir, stop_hash: &str) -> (PathBuf, PathBuf, PathBuf) {
        let home = dir.join("codex");
        let cwd = dir.join("project");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        let hooks = home.join("hooks.json");
        std::fs::write(
            &hooks,
            r#"{"hooks":{
  "PreToolUse":[{"matcher":"^(Bash|apply_patch)$","hooks":[{"type":"command","command":"CAS_HOOK_HARNESS=codex cas hook PreToolUse","timeout":120}]}],
  "Stop":[{"hooks":[{"type":"command","command":"CAS_HOOK_HARNESS=codex cas hook Stop","timeout":60}]}],
  "UserPromptSubmit":[{"hooks":[{"type":"command","command":"/opt/operator/audit.sh","timeout":5}]}],
  "PostToolUse":[{"matcher":"^Bash$","hooks":[{"type":"command","command":"/opt/operator/off.sh","timeout":5}]}]
}}"#,
        )
        .unwrap();
        let key = |event: &str| format!("{}:{event}:0:0", hooks.display());
        let pre = codex_command_hook_hash(
            "pre_tool_use",
            "^(Bash|apply_patch)$",
            "CAS_HOOK_HARNESS=codex cas hook PreToolUse",
            120,
            false,
        );
        let config = home.join("config.toml");
        std::fs::write(
            &config,
            format!(
                "[projects.\"{}\"]\ntrust_level = \"trusted\"\n\n\
                 [hooks.state.\"{}\"]\ntrusted_hash = \"{pre}\"\n\n\
                 [hooks.state.\"{}\"]\ntrusted_hash = \"{stop_hash}\"\n\n\
                 [hooks.state.\"{}\"]\nenabled = false\n",
                cwd.display(),
                key("pre_tool_use"),
                key("stop"),
                key("post_tool_use"),
            ),
        )
        .unwrap();
        (config, home, cwd)
    }

    #[test]
    fn launch_readiness_blocks_a_modified_cas_handler_cas_d0fe() {
        // A Stop trust hash written with the pre-cas-d0fe identity (matcher
        // included) is `modified` to Codex 0.163, which parks the pane.
        let dir = TempDir::new();
        let stale = {
            let identity = serde_json::json!({
                "event_name": "stop", "matcher": "",
                "hooks": [{"type": "command", "command": "CAS_HOOK_HARNESS=codex cas hook Stop", "timeout": 60, "async": false}],
            });
            format!(
                "sha256:{:x}",
                Sha256::digest(serde_json::to_vec(&identity).unwrap())
            )
        };
        let (config, home, cwd) = readiness_fixture(&dir, &stale);
        let readiness = codex_launch_readiness(&config, &home, &cwd).unwrap();
        let blocking = readiness.blocking();
        assert_eq!(blocking.len(), 1, "{blocking:?}");
        assert!(
            blocking[0].contains(":stop:0:0") && blocking[0].contains("Modified"),
            "{blocking:?}"
        );

        // cas-015d: display paths safely while retaining Codex's raw trust key.
        for (suffix, visible) in [("\u{202e}", "\\u{202e}"), ("\u{200b}", "\\u{200b}")] {
            let unsafe_home = dir.join(&format!("codex{suffix}home"));
            let unsafe_cwd = dir.join(&format!("project{suffix}cwd"));
            std::fs::create_dir_all(&unsafe_home).unwrap();
            std::fs::create_dir_all(&unsafe_cwd).unwrap();
            let hooks = unsafe_home.join("hooks.json");
            std::fs::copy(home.join("hooks.json"), &hooks).unwrap();
            let unsafe_config = unsafe_home.join("config.toml");
            let config_before = std::fs::read_to_string(&config).unwrap()
                .replace(home.to_str().unwrap(), unsafe_home.to_str().unwrap());
            std::fs::write(&unsafe_config, &config_before).unwrap();
            let readiness = codex_launch_readiness(&unsafe_config, &unsafe_home, &unsafe_cwd).unwrap();
            let key = format!("{}:pre_tool_use:0:0", hooks.display());
            assert!(readiness.hooks.iter().any(|hook| hook.key == key && hook.trust == CodexHookTrust::Trusted), "raw identity must still find its stored hash");
            let blocking = readiness.blocking();
            assert!(blocking.iter().any(|reason| reason.contains(&format!("project{visible}cwd")) && reason.contains("is not trusted")), "{blocking:?}");
            assert!(blocking.iter().all(|reason| !reason.contains(suffix)), "{blocking:?}");
            std::fs::write(&hooks, "{invalid JSON").unwrap();
            let error = codex_launch_readiness(&unsafe_config, &unsafe_home, &unsafe_cwd).unwrap_err().to_string();
            assert!(error.contains(&format!("codex{visible}home")) && error.contains("failed to parse"), "{error:?}");
            assert!(!error.contains(suffix), "{error:?}");
            assert_eq!(std::fs::read_to_string(&unsafe_config).unwrap(), config_before);
            assert_eq!(std::fs::read_to_string(&hooks).unwrap(), "{invalid JSON");
        }
    }

    #[test]
    fn launch_readiness_reports_but_never_blocks_on_foreign_hooks_cas_d0fe() {
        let dir = TempDir::new();
        let stop = codex_command_hook_hash(
            "stop",
            "",
            "CAS_HOOK_HARNESS=codex cas hook Stop",
            60,
            false,
        );
        let (config, home, cwd) = readiness_fixture(&dir, &stop);
        let readiness = codex_launch_readiness(&config, &home, &cwd).unwrap();
        assert!(
            readiness.blocking().is_empty(),
            "{:?}",
            readiness.blocking()
        );
        // The untrusted operator hook is reported for review; the disabled one
        // is inactive and neither blocks nor needs review.
        let review: Vec<_> = readiness
            .foreign_needing_review()
            .iter()
            .map(|h| h.command.clone())
            .collect();
        assert_eq!(review, ["/opt/operator/audit.sh"]);
        assert!(readiness.hooks.iter().any(|h| h.command == "/opt/operator/off.sh" && h.trust == CodexHookTrust::Disabled));
        let config_before = std::fs::read_to_string(&config).unwrap();
        codex_launch_readiness(&config, &home, &cwd).unwrap();
        // pin: readiness is read-only, so the config bytes must be identical.
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            config_before,
            "readiness never writes"
        );
    }

    #[test]
    fn hook_trust_registration_uses_actual_indexes_and_is_idempotent() {
        let dir = TempDir::new();
        let config_path = dir.join("config.toml");
        let hooks_path = dir.join("project/.codex/hooks.json");
        std::fs::create_dir_all(hooks_path.parent().unwrap()).unwrap();
        // A non-CAS matcher group ahead of PreToolUse proves we use Codex's
        // positional trust key, rather than assuming every CAS handler is 0:0.
        std::fs::write(
            &hooks_path,
            r#"{
  "hooks": {
    "PreToolUse": [
      {"matcher":"^Read$","hooks":[{"type":"command","command":"operator hook"}]},
      {"matcher":"^Bash$","hooks":[{"type":"command","command":"CAS_HOOK_HARNESS=codex cas hook PreToolUse","timeout":3}]}
    ],
    "PostToolUse": [
      {"matcher":"^Bash$","hooks":[{"type":"command","command":"cas hook PostToolUse","timeout":3}]}
    ]
  }
}"#,
        )
        .unwrap();
        std::fs::write(&config_path, "# operator content\nmodel = \"gpt-5\"\n").unwrap();

        assert!(ensure_cas_hooks_trusted_in(&config_path, &hooks_path).unwrap());
        let after_first = std::fs::read_to_string(&config_path).unwrap();
        let hook_source = std::fs::read_to_string(&hooks_path).unwrap();
        let hook_value: serde_json::Value = serde_json::from_str(&hook_source).unwrap();
        assert_eq!(
            hook_value["hooks"]["PreToolUse"][1]["hooks"][0]["timeout"],
            CODEX_PRE_TOOL_TIMEOUT_SECS
        );
        assert_eq!(
            hook_value["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "operator hook"
        );
        assert_eq!(
            hook_value["hooks"]["PostToolUse"][0]["hooks"][0]["timeout"],
            3
        );
        let entries = cas_hook_trust_entries(&hooks_path).unwrap();
        assert_eq!(
            entries[0].key,
            format!("{}:pre_tool_use:1:0", hooks_path.display())
        );
        assert_eq!(
            entries[1].key,
            format!("{}:post_tool_use:0:0", hooks_path.display())
        );
        for entry in &entries {
            assert!(hook_state_is_trusted(&after_first, entry).unwrap());
        }
        assert!(after_first.contains("# operator content"));

        assert!(!ensure_cas_hooks_trusted_in(&config_path, &hooks_path).unwrap());
        assert_eq!(std::fs::read_to_string(&config_path).unwrap(), after_first);
        assert_eq!(std::fs::read_to_string(&hooks_path).unwrap(), hook_source);
    }

    #[test]
    fn missing_or_empty_account_hooks_fail_naming_the_file_cas_10af() {
        // cas-10af: an absent account hooks.json was read as "" and surfaced
        // only as "EOF while parsing a value at line 1 column 0".
        let dir = TempDir::new();
        let config_path = dir.join("config.toml");
        let hooks_path = dir.join("hooks.json");
        for present in [false, true] {
            if present {
                std::fs::write(&hooks_path, "").unwrap();
            }
            let message = ensure_cas_hooks_trusted_in(&config_path, &hooks_path)
                .unwrap_err()
                .to_string();
            assert!(
                message.starts_with(&format!(
                    "codex_hooks_json_missing: {} ",
                    hooks_path.display()
                )),
                "{message}"
            );
        }
    }

    const CURRENT_HOOKS: &str = r#"{"hooks":{
  "PostToolUse":[{"matcher":"^Bash$","hooks":[{"type":"command","command":"cas hook PostToolUse","timeout":3}]}],
  "PreToolUse":[{"matcher":"^(Bash|apply_patch)$","hooks":[{"type":"command","command":"CAS_HOOK_HARNESS=codex cas hook PreToolUse","timeout":120}]}]
}}"#;

    /// A main checkout plus a linked worktree, each with the given hooks.json.
    fn linked_pair(dir: &TempDir, main_hooks: &str, own_hooks: &str) -> (PathBuf, PathBuf) {
        let main = dir.join("main");
        let linked = dir.join("linked");
        std::fs::create_dir_all(main.join(".git/worktrees/w")).unwrap();
        std::fs::write(main.join(".git/worktrees/w/commondir"), "../..\n").unwrap();
        std::fs::create_dir_all(linked.join(".codex")).unwrap();
        std::fs::write(
            linked.join(".git"),
            format!("gitdir: {}\n", main.join(".git/worktrees/w").display()),
        )
        .unwrap();
        std::fs::create_dir_all(main.join(".codex")).unwrap();
        std::fs::write(main.join(".codex/hooks.json"), main_hooks).unwrap();
        std::fs::write(linked.join(".codex/hooks.json"), own_hooks).unwrap();
        (main, linked)
    }

    #[test]
    fn linked_worktree_trusts_the_main_checkout_hooks_codex_0_162_loads() {
        // cas-4984: codex-cli 0.162.1 `hooks/list` for a linked worktree cwd
        // listed only `<main checkout>/.codex/hooks.json`, with these hashes.
        let dir = TempDir::new();
        let (main, linked) = linked_pair(&dir, CURRENT_HOOKS, CURRENT_HOOKS);
        let main_hooks = main.join(".codex/hooks.json");
        let config_path = dir.join("config.toml");
        prepare_codex_project_hooks_in(&config_path, &linked).unwrap();
        assert_eq!(std::fs::read_to_string(&main_hooks).unwrap(), CURRENT_HOOKS);
        let config: toml::Value =
            toml::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
        let trusted = |event: &str| {
            config["hooks"]["state"][format!("{}:{event}:0:0", main_hooks.display()).as_str()]
                ["trusted_hash"]
                .as_str()
                .unwrap()
                .to_string()
        };
        assert_eq!(
            trusted("pre_tool_use"),
            "sha256:06a6a2b38e68da7193b1f3d10f21ca73dde9bc974cd5020e0164fd23d5af0253"
        );
        assert_eq!(
            trusted("post_tool_use"),
            "sha256:94c125201a0019ec158d7abb96cd3fc0d8282bb499aa506f202ce794748cdb2b"
        );

        // Identical tracked files stay current even when the worker needs a
        // local timeout upgrade. Trust the unchanged main file as it stands.
        let short_budget = CURRENT_HOOKS.replace("\"timeout\":120", "\"timeout\":3");
        std::fs::write(&main_hooks, &short_budget).unwrap();
        let own_hooks = linked.join(".codex/hooks.json");
        std::fs::write(&own_hooks, &short_budget).unwrap();
        prepare_codex_project_hooks_in(&config_path, &linked).unwrap();
        assert_eq!(std::fs::read_to_string(&main_hooks).unwrap(), short_budget);
        let own: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&own_hooks).unwrap()).unwrap();
        assert_eq!(own["hooks"]["PreToolUse"][0]["hooks"][0]["timeout"], 120);
        let config = std::fs::read_to_string(&config_path).unwrap();
        for path in [&main_hooks, &own_hooks] {
            for entry in cas_hook_trust_entries(path).unwrap() {
                assert!(hook_state_is_trusted(&config, &entry).unwrap());
            }
        }

        // cas-6591: an unparseable main-checkout file is a named failure.
        std::fs::write(&main_hooks, "{ not json").unwrap();
        let message = prepare_codex_project_hooks_in(&config_path, &linked)
            .unwrap_err()
            .to_string();
        assert!(
            message.starts_with("codex_hooks_json_invalid: "),
            "{message}"
        );
        assert!(
            message.contains(&main_hooks.display().to_string()),
            "{message}"
        );
    }

    #[test]
    fn stale_main_checkout_hooks_fail_the_spawn_untrusted_and_unrewritten() {
        // cas-6591: observed with codex-cli 0.162.1 hooks/list, a main checkout
        // with the older body (PreToolUse `^Bash$`, timeout 3) trusted as it
        // stands runs that stale guard for the linked worker. Refuse it instead.
        let stale = CURRENT_HOOKS
            .replace("^(Bash|apply_patch)$", "^Bash$")
            .replace("\"timeout\":120", "\"timeout\":3");
        let dir = TempDir::new();
        let (plain_main, linked) = linked_pair(&dir, &stale, CURRENT_HOOKS);
        let main = dir.join("main 'quote' {a,b}!e $(touch${IFS}SUBSTITUTED)");
        std::fs::rename(&plain_main, &main).unwrap();
        std::fs::write(
            linked.join(".git"),
            format!("gitdir: {}\n", main.join(".git/worktrees/w").display()),
        )
        .unwrap();
        let main_hooks = main.join(".codex/hooks.json");
        let config_path = dir.join("config.toml");
        let message = prepare_codex_project_hooks_in(&config_path, &linked)
            .unwrap_err()
            .to_string();
        assert!(
            message.starts_with("codex_main_checkout_hooks_stale: "),
            "{message}"
        );
        assert!(
            message.contains(&main_hooks.display().to_string()),
            "{message}"
        );
        // Exercise both printed alternatives in interactive Bash with history
        // expansion enabled. Git is the subprocess boundary, not a parser fake.
        let commands: Vec<_> = message
            .split('`')
            .filter(|part| part.starts_with("git -C "))
            .collect();
        assert_eq!(commands.len(), 2, "{message}");
        for (command, tail) in commands
            .iter()
            .zip([vec!["pull"], vec!["checkout", ".codex/hooks.json"]])
        {
            use std::io::Write;
            let mut shell = std::process::Command::new("bash")
                .args(["--noprofile", "--norc", "-i"])
                .env("HISTFILE", "/dev/null")
                .env("PS1", "")
                .current_dir(&dir.0)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            writeln!(shell.stdin.take().unwrap(),
                    "set -H\nhistory -s 'echo history-expanded'\ngit() {{ printf '%s\\0' \"$@\"; }}\n{command}\nexit $?").unwrap();
            let output = shell.wait_with_output().unwrap();
            let args: Vec<_> = output
                .stdout
                .split(|byte| *byte == 0)
                .filter(|arg| !arg.is_empty())
                .collect();
            let main_text = main.to_str().unwrap();
            let mut expected = vec![b"-C".as_slice(), main_text.as_bytes()];
            expected.extend(tail.iter().map(|arg| arg.as_bytes()));
            assert!(
                output.status.success(),
                "printed remedy failed: {command}: {output:?}"
            );
            assert_eq!(args, expected, "printed remedy changed argv: {command}");
        }
        assert!(
            !dir.join("SUBSTITUTED").exists(),
            "a printed remedy ran path substitution"
        );
        assert_eq!(std::fs::read_to_string(&main_hooks).unwrap(), stale);
        let config = std::fs::read_to_string(&config_path).unwrap_or_default();
        assert!(
            !config.contains(&main_hooks.display().to_string()),
            "{config}"
        );

        // A malformed CAS definition must identify its cause, rather than
        // prescribing a checkout update for an ordinary stale body.
        let mut broken: serde_json::Value = serde_json::from_str(CURRENT_HOOKS).unwrap();
        broken["hooks"]
            .as_object_mut()
            .unwrap()
            .remove("PostToolUse");
        let broken = serde_json::to_string(&broken).unwrap();
        std::fs::write(&main_hooks, &broken).unwrap();
        let error = prepare_codex_project_hooks_in(&config_path, &linked).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        let message = error.to_string();
        assert!(message.starts_with("codex_hooks_invalid: "), "{message}");
        assert!(
            message.contains("missing CAS PostToolUse hook groups"),
            "{message}"
        );
        assert!(
            message.contains(&main_hooks.display().to_string()),
            "{message}"
        );
        assert_eq!(std::fs::read_to_string(&main_hooks).unwrap(), broken);
        let config = std::fs::read_to_string(&config_path).unwrap_or_default();
        assert!(
            !config.contains(&main_hooks.display().to_string()),
            "{config}"
        );

        // cas-4e22: quoted paths can still inject controls or reorder the
        // diagnostic. Refuse a pasteable command rather than change its argv.
        let mut previous_main = main;
        for (suffix, visible, prefix) in [
            ("\u{85}\u{202e}", "\\u{85}\\u{202e}", "codex_hooks_invalid:"),
            ("\u{202e}", "\\u{202e}", "codex_main_checkout_hooks_stale:"),
            ("\u{2028}", "\\u{2028}", "codex_main_checkout_hooks_stale:"),
            ("\u{200b}", "\\u{200b}", "codex_main_checkout_hooks_stale:"),
        ] {
            let unsafe_main = dir.join(&format!("main{suffix}folder"));
            std::fs::rename(&previous_main, &unsafe_main).unwrap();
            std::fs::write(unsafe_main.join(".codex/hooks.json"), &stale).unwrap();
            std::fs::write(
                linked.join(".git"),
                format!(
                    "gitdir: {}\n",
                    unsafe_main.join(".git/worktrees/w").display()
                ),
            )
            .unwrap();
            let message = prepare_codex_project_hooks_in(&config_path, &linked)
                .unwrap_err()
                .to_string();
            assert!(message.starts_with(prefix), "{message:?}");
            assert!(
                message.contains(&format!("main{visible}folder")),
                "{message:?}"
            );
            assert!(!message.contains(['\u{85}', '\u{202e}']), "{message:?}");
            if prefix == "codex_main_checkout_hooks_stale:" {
                assert!(message.contains("no copyable command"), "{message:?}");
            }
            assert!(!message.contains("`git -C "), "{message:?}");
            // pin: refusing an unsafe remedy must leave the tracked hook bytes intact.
            assert_eq!(
                std::fs::read_to_string(unsafe_main.join(".codex/hooks.json")).unwrap(),
                stale
            );
            // cas-98f9: nested trust errors and malformed JSON must not
            // reintroduce the raw path after an escaped outer label.
            let unsafe_hooks = unsafe_main.join(".codex/hooks.json");
            for contents in [&broken, "{invalid JSON"] {
                std::fs::write(&unsafe_hooks, contents).unwrap();
                let message = prepare_codex_project_hooks_in(&config_path, &linked)
                    .unwrap_err()
                    .to_string();
                assert!(
                    message.contains(&format!("main{visible}folder")),
                    "{message:?}"
                );
                assert!(
                    !message.contains(['\u{85}', '\u{202e}', '\u{2028}', '\u{200b}']),
                    "{message:?}"
                );
                assert_eq!(std::fs::read_to_string(&unsafe_hooks).unwrap(), contents);
            }
            previous_main = unsafe_main;
        }
    }

    #[test]
    fn hook_trust_registration_refreshes_only_its_stale_hash() {
        let dir = TempDir::new();
        let config_path = dir.join("config.toml");
        let hooks_path = dir.join("project/.codex/hooks.json");
        std::fs::create_dir_all(hooks_path.parent().unwrap()).unwrap();
        std::fs::write(
            &hooks_path,
            r#"{"hooks":{"PreToolUse":[{"matcher":"^Bash$","hooks":[{"type":"command","command":"CAS_HOOK_HARNESS=codex cas hook PreToolUse","timeout":3}]}],"PostToolUse":[{"matcher":"^Bash$","hooks":[{"type":"command","command":"cas hook PostToolUse","timeout":3}]}]}}"#,
        )
        .unwrap();
        ensure_cas_hooks_trusted_in(&config_path, &hooks_path).unwrap();
        let entries = cas_hook_trust_entries(&hooks_path).unwrap();
        let original = std::fs::read_to_string(&config_path).unwrap();
        let stale = original.replacen(&entries[0].hash, "sha256:stale", 1);
        std::fs::write(&config_path, stale).unwrap();

        assert!(ensure_cas_hooks_trusted_in(&config_path, &hooks_path).unwrap());
        let refreshed = std::fs::read_to_string(&config_path).unwrap();
        assert!(hook_state_is_trusted(&refreshed, &entries[0]).unwrap());
        assert!(hook_state_is_trusted(&refreshed, &entries[1]).unwrap());
        assert_eq!(
            refreshed.matches(&entries[0].key).count(),
            1,
            "refresh must update the owned block rather than append a duplicate"
        );
    }

    #[test]
    fn inline_projects_table_entry_is_not_duplicated() {
        // The shape Codex's own config can take; appending here would produce a
        // duplicate-table parse error for the whole file.
        let dir = TempDir::new();
        let config_path = dir.join("config.toml");
        let workdir = dir.join("proj");
        std::fs::create_dir_all(&workdir).unwrap();
        let original = format!(
            "[projects]\n\"{}\" = {{ trust_level = \"trusted\" }}\n",
            workdir.to_string_lossy()
        );
        std::fs::write(&config_path, &original).unwrap();

        let outcome = ensure_project_trusted_in(&config_path, &workdir).unwrap();
        assert_eq!(outcome, CodexTrustOutcome::AlreadyPresent);
        assert_eq!(std::fs::read_to_string(&config_path).unwrap(), original);
    }

    #[test]
    fn unparseable_operator_config_is_left_untouched() {
        let dir = TempDir::new();
        let config_path = dir.join("config.toml");
        let workdir = dir.join("proj");
        std::fs::create_dir_all(&workdir).unwrap();
        let original = "this is [not( toml\n";
        std::fs::write(&config_path, original).unwrap();

        let outcome = ensure_project_trusted_in(&config_path, &workdir).unwrap();
        assert!(
            matches!(outcome, CodexTrustOutcome::Skipped(_)),
            "must refuse to modify an unparseable config, got {outcome:?}"
        );
        assert_eq!(std::fs::read_to_string(&config_path).unwrap(), original);
    }

    #[test]
    fn preserves_an_explicit_untrusted_decision_and_blocks_launch_precondition() {
        let dir = TempDir::new();
        let config_path = dir.join("config.toml");
        let workdir = dir.join("proj");
        std::fs::create_dir_all(&workdir).unwrap();
        let original = format!(
            "[projects.\"{}\"]\ntrust_level = \"untrusted\"\n",
            workdir.to_string_lossy()
        );
        std::fs::write(&config_path, &original).unwrap();

        let outcome = ensure_project_trusted_in(&config_path, &workdir).unwrap();
        assert!(matches!(outcome, CodexTrustOutcome::Skipped(_)));
        assert_eq!(std::fs::read_to_string(&config_path).unwrap(), original);
    }

    #[test]
    fn relative_workdir_is_skipped() {
        let dir = TempDir::new();
        let config_path = dir.join("config.toml");
        let outcome = ensure_project_trusted_in(&config_path, Path::new("relative/dir")).unwrap();
        assert!(matches!(outcome, CodexTrustOutcome::Skipped(_)));
        assert!(!config_path.exists());
    }

    #[test]
    fn control_character_paths_are_refused() {
        // A newline in a path is legal on Linux and would otherwise inject a
        // whole table (e.g. [mcp_servers.evil]) into the operator's config.
        let dir = TempDir::new();
        let config_path = dir.join("config.toml");
        let workdir = dir.join("proj\n[mcp_servers.evil]\ncommand = \"sh\"\n");

        let outcome = ensure_project_trusted_in(&config_path, &workdir).unwrap();
        assert!(
            matches!(outcome, CodexTrustOutcome::Skipped(_)),
            "must refuse a control-character path, got {outcome:?}"
        );
        assert!(!config_path.exists());
        assert!(path_is_unsafe_key("/a/b\nc"));
        assert!(!path_is_unsafe_key("/a/b c"));
    }

    #[cfg(unix)]
    #[test]
    fn existing_file_mode_is_preserved() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new();
        let config_path = dir.join("config.toml");
        let workdir = dir.join("proj");
        std::fs::create_dir_all(&workdir).unwrap();
        std::fs::write(&config_path, "model = \"gpt-5\"\n").unwrap();
        std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600)).unwrap();

        ensure_project_trusted_in(&config_path, &workdir).unwrap();
        let mode = std::fs::metadata(&config_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o600,
            "a 0600 config (it can hold API keys) must not widen on rewrite"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_config_keeps_its_link() {
        let dir = TempDir::new();
        let real = dir.join("dotfiles-config.toml");
        let link = dir.join("config.toml");
        let workdir = dir.join("proj");
        std::fs::create_dir_all(&workdir).unwrap();
        std::fs::write(&real, "model = \"gpt-5\"\n").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();

        ensure_project_trusted_in(&link, &workdir).unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "a symlinked config.toml must stay a symlink, not be replaced by a regular file"
        );
        assert!(
            config_has_project(
                &std::fs::read_to_string(&real).unwrap(),
                &workdir.to_string_lossy()
            )
            .unwrap(),
            "the entry must land in the symlink target"
        );
    }

    #[test]
    fn codex_home_prefers_env_override() {
        let mut env = crate::test_env_guard::TestEnvGuard::new();
        env.set("CODEX_HOME", "/tmp/cas-codex-home-test");
        assert_eq!(
            codex_home(),
            Some(PathBuf::from("/tmp/cas-codex-home-test"))
        );
    }

    /// Minimal self-cleaning temp dir for the Codex trust fixtures.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let dir =
                std::env::temp_dir().join(format!("cas-codex-trust-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn join(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
