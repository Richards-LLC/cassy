//! Read-side guard for the operator's Neon and Vercel credential stores.
//!
//! Resolve tool path arguments independently, without opening the target. Shell
//! commands are split into argv words first so nearby whitespace cannot turn
//! unrelated arguments into a path.

use std::path::{Component, Path, PathBuf};

use cas_core::hooks::types::HookInput;
use serde_json::Value;

const STORE_SUFFIXES: &[&str] = &[
    ".config/neonctl",
    ".local/share/com.vercel.cli",
    "Library/Application Support/com.vercel.cli",
    ".config/com.vercel.cli",
];

const PATH_KEYS: &[&str] = &[
    "file_path",
    "path",
    "notebook_path",
    "directory",
    "directories",
    "paths",
    "files",
];

pub(super) fn references_store(input: &HookInput, tool: &str) -> bool {
    let Some(tool_input) = input.tool_input.as_ref() else {
        return false;
    };
    let cwd = Path::new(&input.cwd);

    if tool.eq_ignore_ascii_case("Bash") {
        let Some(command) = tool_input.get("command").and_then(Value::as_str) else {
            return false;
        };
        let recursive = recursively_scans_paths(command);
        return super::pre_tool::shell_statement_words(command)
            .iter()
            .flat_map(|words| words.iter())
            .any(|word| {
                path_references_store(word, cwd, recursive)
                    || script_constructs_store_path(word, cwd)
            })
            || super::pre_tool::shell_heredoc_bodies(command)
                .iter()
                .any(|body| script_constructs_store_path(body, cwd));
    }

    if ![
        "Read",
        "Grep",
        "Glob",
        "Edit",
        "MultiEdit",
        "NotebookRead",
        "NotebookEdit",
    ]
    .iter()
    .any(|name| tool.eq_ignore_ascii_case(name))
    {
        return false;
    }
    let include_parent = matches!(tool.to_ascii_lowercase().as_str(), "grep" | "glob");
    let mut paths = Vec::new();
    collect_path_fields(tool_input, &mut paths);
    paths
        .into_iter()
        .any(|path| path_references_store(path, cwd, include_parent))
}

fn collect_path_fields<'a>(value: &'a Value, paths: &mut Vec<&'a str>) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if PATH_KEYS.contains(&key.as_str()) {
                    collect_strings(value, paths);
                } else {
                    collect_path_fields(value, paths);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_path_fields(value, paths);
            }
        }
        _ => {}
    }
}

fn collect_strings<'a>(value: &'a Value, paths: &mut Vec<&'a str>) {
    match value {
        Value::String(path) => paths.push(path),
        Value::Array(values) => {
            for value in values {
                collect_strings(value, paths);
            }
        }
        Value::Object(object) => {
            for value in object.values() {
                collect_strings(value, paths);
            }
        }
        _ => {}
    }
}

fn recursively_scans_paths(command: &str) -> bool {
    super::pre_tool::shell_invocations(command)
        .into_iter()
        .any(|(name, args)| match name.as_str() {
            "rg" | "find" => true,
            "grep" | "egrep" | "fgrep" => args.iter().any(|arg| {
                matches!(arg.as_str(), "--recursive" | "-r" | "-R")
                    || (arg.starts_with('-')
                        && !arg.starts_with("--")
                        && arg[1..].contains(['r', 'R']))
            }),
            "cp" => args.iter().any(|arg| {
                arg == "--recursive"
                    || (arg.starts_with('-')
                        && !arg.starts_with("--")
                        && arg[1..].contains(['r', 'R']))
            }),
            _ => false,
        })
}

fn path_references_store(reference: &str, cwd: &Path, include_parent: bool) -> bool {
    let reference = reference.trim_matches(|ch: char| matches!(ch, '\'' | '"' | '`' | ',' | ';'));
    if reference.is_empty() {
        return false;
    }
    let roots = store_roots(cwd);
    let path = if include_parent {
        path_prefix_before_glob(reference)
    } else {
        reference
    };
    if let Some(candidate) = resolve_path(path, cwd) {
        if roots.iter().any(|root| {
            candidate.starts_with(root) || (include_parent && root.starts_with(&candidate))
        }) {
            return true;
        }
    }
    alias_references_store(path, include_parent)
}

fn path_prefix_before_glob(reference: &str) -> &str {
    let Some(glob_at) = reference.find(['*', '?', '[']) else {
        return reference;
    };
    reference[..glob_at]
        .rfind('/')
        .map(|slash| &reference[..slash])
        .unwrap_or(".")
}

fn store_roots(cwd: &Path) -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|home| home.join(".config")));
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|home| home.join(".local/share")));
    let mut roots = Vec::new();
    if let Some(home) = &home {
        roots.extend(STORE_SUFFIXES.iter().map(|suffix| home.join(suffix)));
    }
    if let Some(config_home) = config_home {
        roots.push(config_home.join("neonctl"));
        roots.push(config_home.join("com.vercel.cli"));
    }
    if let Some(data_home) = data_home {
        roots.push(data_home.join("com.vercel.cli"));
    }
    if let Some(vercel_root) = std::env::var_os("VERCEL_CONFIG_DIR") {
        roots.push(PathBuf::from(vercel_root));
    }
    roots
        .into_iter()
        .map(|root| lexical_absolute(&root, cwd))
        .collect()
}

fn resolve_path(reference: &str, cwd: &Path) -> Option<PathBuf> {
    let reference = expand_path_prefix(reference)?;
    let path = PathBuf::from(reference);
    Some(lexical_absolute(&path, cwd))
}

fn expand_path_prefix(reference: &str) -> Option<String> {
    if reference == "~" || reference.starts_with("~/") {
        let home = std::env::var_os("HOME")?;
        let suffix = reference.strip_prefix("~/").unwrap_or("");
        return Some(
            PathBuf::from(home)
                .join(suffix)
                .to_string_lossy()
                .into_owned(),
        );
    }
    for (variable, value) in [
        ("HOME", std::env::var_os("HOME").map(PathBuf::from)),
        (
            "XDG_CONFIG_HOME",
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config"))
                }),
        ),
        (
            "XDG_DATA_HOME",
            std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
                }),
        ),
        (
            "VERCEL_CONFIG_DIR",
            std::env::var_os("VERCEL_CONFIG_DIR").map(PathBuf::from),
        ),
    ] {
        let Some(value) = value else { continue };
        for alias in [format!("${variable}"), format!("${{{variable}}}")] {
            if reference == alias {
                return Some(value.to_string_lossy().into_owned());
            }
            if let Some(rest) = reference.strip_prefix(&format!("{alias}/")) {
                return Some(value.join(rest).to_string_lossy().into_owned());
            }
        }
    }
    Some(reference.to_string())
}

fn lexical_absolute(path: &Path, cwd: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

fn alias_references_store(reference: &str, include_parent: bool) -> bool {
    let reference = reference.replace('\\', "/");
    for (alias, suffixes) in [
        ("$HOME/", STORE_SUFFIXES.to_vec()),
        ("${HOME}/", STORE_SUFFIXES.to_vec()),
        ("~/", STORE_SUFFIXES.to_vec()),
        ("$XDG_CONFIG_HOME/", vec!["neonctl", "com.vercel.cli"]),
        ("${XDG_CONFIG_HOME}/", vec!["neonctl", "com.vercel.cli"]),
        ("$XDG_DATA_HOME/", vec!["com.vercel.cli"]),
        ("${XDG_DATA_HOME}/", vec!["com.vercel.cli"]),
        ("$VERCEL_CONFIG_DIR/", vec![""]),
        ("${VERCEL_CONFIG_DIR}/", vec![""]),
    ] {
        let Some(rest) = reference.strip_prefix(alias) else {
            continue;
        };
        if suffixes.iter().any(|suffix| {
            (suffix.is_empty() && !rest.is_empty())
                || rest == *suffix
                || rest.starts_with(&format!("{suffix}/"))
                || (include_parent && suffix.starts_with(&format!("{rest}/")))
        }) {
            return true;
        }
    }
    false
}

fn script_constructs_store_path(script: &str, cwd: &Path) -> bool {
    let strings = quoted_strings(script);
    if strings
        .iter()
        .any(|part| path_references_store(part, cwd, false))
    {
        return true;
    }
    let source = script.to_ascii_lowercase();
    let reads_home = [
        "os.homedir",
        "path.home",
        "homedir",
        "os.environ",
        "os.getenv",
        "process.env.home",
        "process.env[\"home\"]",
        "process.env['home']",
        "$home",
        "${home}",
    ]
    .iter()
    .any(|marker| source.contains(marker));
    if !reads_home {
        return false;
    }
    let components = strings.join("/").to_ascii_lowercase();
    [
        ".config/neonctl/credentials.json",
        ".local/share/com.vercel.cli/auth.json",
        "library/application support/com.vercel.cli/auth.json",
        ".config/com.vercel.cli/auth.json",
    ]
    .iter()
    .any(|suffix| components.contains(suffix))
        || (["xdg_config_home", "xdg_data_home", "vercel_config_dir"]
            .iter()
            .any(|marker| source.contains(marker))
            && ["neonctl/credentials.json", "com.vercel.cli/auth.json"]
                .iter()
                .any(|suffix| components.contains(suffix)))
}

fn quoted_strings(source: &str) -> Vec<String> {
    let mut strings = Vec::new();
    let mut chars = source.chars().peekable();
    while let Some(quote @ ('\'' | '"')) = chars.next() {
        let mut value = String::new();
        let mut escaped = false;
        for ch in chars.by_ref() {
            if escaped {
                value.push(ch);
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                strings.push(value);
                break;
            } else {
                value.push(ch);
            }
        }
    }
    strings
}
