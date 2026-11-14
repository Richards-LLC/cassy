//! Read-side guard for the operator's Neon and Vercel credential stores.
//!
//! Resolve tool path arguments independently, without opening the target. Shell
//! commands are split into argv words first so nearby whitespace cannot turn
//! unrelated arguments into a path.

use std::path::{Component, Path, PathBuf};

use cas_core::hooks::types::HookInput;
use serde_json::Value;

use cas_pty::store_isolation::HOME_STORE_SUFFIXES as STORE_SUFFIXES;
use super::pre_tool::{SHELL_MAX_DEPTH, ShellInput, shell_input};

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
        return shell_references_store(command, cwd, 0);
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

fn shell_references_store(command: &str, initial_cwd: &Path, depth: usize) -> bool {
    if depth > SHELL_MAX_DEPTH {
        // This scope is outside the shared admission grammar. Let that guard
        // report shell.depth instead of inventing a credential-store access.
        return false;
    }
    let mut cwd = initial_cwd.to_path_buf();
    let mut directory_stack = Vec::new();
    // Nested shells are not standalone listings, so their payloads never
    // reach here with inert names.
    let names_inert = depth == 0 && listed_names_stay_inert(command);
    for (words, redirects) in super::pre_tool::shell_statement_io(command, depth) {
        if redirects.iter().any(|path| path_references_store(path, &cwd, false))
            || super::pre_tool::shell_wrapper_read_paths(&words).iter()
                .any(|(path, wrapper)| path_references_store(path, &wrapper_cwd(&words[..*wrapper], &cwd), false))
        { return true; }
        let Some(command_index) = super::pre_tool::unwrapped_command_index(&words) else {
            continue;
        };
        let executable = Path::new(&words[command_index])
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&words[command_index])
            .to_ascii_lowercase();
        let args = &words[command_index + 1..];

        // Inspect shell payloads as nested statement lists so a `cd` inside a
        // `sh -c` or `bash -c` does not leak into the caller's working dir.
        if matches!(executable.as_str(), "sh" | "bash" | "zsh" | "dash" | "ksh") {
            let reads_store = match shell_input(args) {
                ShellInput::Command(at) => args.get(at)
                    .is_some_and(|payload| shell_references_store(payload, &cwd, depth + 1)),
                ShellInput::Script(at) => args.get(at)
                    .is_some_and(|path| path_references_store(path, &cwd, false)),
                ShellInput::Stdin | ShellInput::Display => false,
            };
            if reads_store {
                return true;
            }
            continue;
        }
        if executable == "eval" {
            if shell_references_store(&args.join(" "), &cwd, depth + 1) {
                return true;
            }
            continue;
        }

        if matches!(executable.as_str(), "cd" | "pushd") {
            if let Some(target) = cd_target(args) {
                if path_references_store(target, &cwd, false) {
                    return true;
                }
                if let Some(next_cwd) = resolve_path(target, &cwd) {
                    if executable == "pushd" {
                        directory_stack.push(cwd.clone());
                    }
                    cwd = next_cwd;
                }
            } else if let Some(home) = std::env::var_os("HOME") {
                if executable == "pushd" {
                    directory_stack.push(cwd.clone());
                }
                cwd = PathBuf::from(home);
            }
            continue;
        }
        if executable == "popd" {
            if let Some(previous) = directory_stack.pop() {
                cwd = previous;
            }
            continue;
        }

        let names_only = names_inert && lists_names_only(&executable, args);
        if statement_references_store(&words, &cwd, names_only) {
            return true;
        }
    }

    super::pre_tool::shell_heredoc_bodies(command)
        .iter()
        .any(|body| script_constructs_store_path(body, &cwd))
}

fn statement_references_store(
    words: &[String],
    cwd: &Path,
    names_only: bool,
) -> bool {
    let Some(command_index) = super::pre_tool::unwrapped_command_index(words) else {
        return false;
    };
    let executable = Path::new(&words[command_index])
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&words[command_index])
        .to_ascii_lowercase();
    let args = &words[command_index + 1..];

    let cwd = &wrapper_cwd(&words[..command_index], cwd);

    let descends = invocation_recursively_reads_directories(&executable, args);
    // A listing descends into a store's parent but opens no file. An operand
    // that names a store itself is still refused below.
    let recursive = descends && !names_only;
    let recursive_operands = recursive_path_operand_indices(&executable, args);
    let store_parent_cwd = descends && cwd_is_store_or_parent(cwd);
    for (index, arg) in args.iter().enumerate() {
        let explicit_path = shell_argument_is_path(&executable, arg, cwd);
        let inferred_recursive_path = store_parent_cwd
            && recursive_operands.contains(&index)
            && path_operand_is_bare_relative(arg);
        if (explicit_path || inferred_recursive_path) && path_references_store(arg, cwd, recursive)
        {
            return true;
        }
    }
    // `zip -R` matches its patterns under the working directory.
    if store_parent_cwd
        && executable == "zip"
        && args.iter().any(|arg| {
            arg == "--recurse-patterns"
                || (arg.starts_with('-') && !arg.starts_with("--") && arg.contains('R'))
        })
        && path_references_store(".", cwd, true)
    {
        return true;
    }
    if recursive
        && store_parent_cwd
        && matches!(executable.as_str(), "rg" | "grep" | "egrep" | "fgrep")
        && recursive_search_defaults_to_cwd(&executable, args)
        // The first positional may be the search pattern, not a file.
        && recursive_operands.len() <= 1
        && path_references_store(".", cwd, true)
    {
        return true;
    }
    is_script_interpreter(&executable)
        && args
            .iter()
            .any(|arg| script_constructs_store_path(arg, cwd))
}

/// The directory the command runs in after its wrapper prefix: `env -C DIR`
/// and `sudo -D DIR` (or `--chdir`) move it. `unwrapped_command_index` skips
/// these wrappers, so their directory change is applied here from the
/// skipped words.
fn wrapper_cwd(prefix: &[String], cwd: &Path) -> PathBuf {
    let mut cwd = cwd.to_path_buf();
    let mut chdir_flag = None;
    let mut index = 0;
    while index < prefix.len() {
        let word = prefix[index].as_str();
        index += 1;
        match Path::new(word).file_name().and_then(|name| name.to_str()) {
            Some("env") => {
                chdir_flag = Some("-C");
                continue;
            }
            Some("sudo") => {
                chdir_flag = Some("-D");
                continue;
            }
            _ => {}
        }
        let Some(short) = chdir_flag else { continue };
        let target = if word == short || word == "--chdir" {
            index += 1;
            prefix.get(index - 1).map(String::as_str)
        } else {
            word.strip_prefix("--chdir=")
                .or_else(|| word.strip_prefix(short).filter(|dir| !dir.is_empty()))
        };
        if let Some(next) = target.and_then(|dir| resolve_path(dir, &cwd)) {
            cwd = next;
        }
    }
    cwd
}

/// `rg --files` and a `find` that runs no command print names without
/// opening files (GH #1838, #1700). The argv is parsed, never searched for a
/// token: every option must be one the listing grammar knows, so an option
/// value (`-e --files`, `--regexp=--files`, `-name -exec`) or a word after
/// `--` cannot stand in for the listing form. Anything else is a read.
fn lists_names_only(name: &str, args: &[String]) -> bool {
    match name {
        "rg" => rg_lists_names_only(args),
        "find" => find_lists_names_only(args),
        _ => false,
    }
}

fn rg_lists_names_only(args: &[String]) -> bool {
    const FLAGS: &[&str] = &[
        "--files", "--hidden", "-.", "--no-ignore", "--no-ignore-vcs", "--no-ignore-parent",
        "--no-ignore-dot", "-u", "-uu", "-uuu", "--follow", "-L", "--null", "-0",
        "--no-messages", "--one-file-system",
    ];
    const VALUES: &[&str] = &[
        "-g", "--glob", "--iglob", "-t", "--type", "-T", "--type-not", "-d", "--max-depth",
        "--sort", "--sortr", "--color",
    ];
    let mut files = false;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "--" {
            break;
        }
        if VALUES.contains(&arg) {
            index += 2;
            continue;
        }
        let known_value = arg
            .split_once('=')
            .is_some_and(|(name, _)| name.starts_with("--") && VALUES.contains(&name));
        if arg.starts_with('-') && !FLAGS.contains(&arg) && !known_value {
            return false;
        }
        files |= arg == "--files";
        index += 1;
    }
    files
}

fn find_lists_names_only(args: &[String]) -> bool {
    const TESTS: &[&str] = &[
        "-print", "-print0", "-prune", "-o", "-or", "-a", "-and", "-not", "!", "(", ")",
        "-empty", "-xdev", "-mount", "-follow", "-depth", "-readable", "-writable",
        "-executable", "-true", "-false", "-nouser", "-nogroup",
    ];
    const VALUED: &[&str] = &[
        "-name", "-iname", "-path", "-ipath", "-wholename", "-iwholename", "-type", "-xtype",
        "-maxdepth", "-mindepth", "-size", "-mtime", "-mmin", "-atime", "-amin", "-ctime",
        "-cmin", "-perm", "-regex", "-iregex", "-regextype", "-user", "-group", "-uid", "-gid",
        "-links", "-inum", "-newer", "-samefile",
    ];
    let mut index = 0;
    while args.get(index).is_some_and(|arg| matches!(arg.as_str(), "-H" | "-L" | "-P")) {
        index += 1;
    }
    while args
        .get(index)
        .is_some_and(|arg| !arg.starts_with('-') && !matches!(arg.as_str(), "!" | "(" | ")"))
    {
        index += 1;
    }
    while let Some(arg) = args.get(index) {
        index += if VALUED.contains(&arg.as_str()) {
            2
        } else if TESTS.contains(&arg.as_str()) {
            1
        } else {
            return false;
        };
    }
    true
}

/// Metadata names remain inert only through proven data-only consumers. The
/// common shell/option projection refuses ambiguous or file-opening flows;
/// the credential path policy below remains independent of that classification.
fn listed_names_stay_inert(command: &str) -> bool {
    super::pre_tool::filename_pipeline_is_data_only(command)
}

fn cwd_is_store_or_parent(cwd: &Path) -> bool {
    store_roots(cwd)
        .iter()
        .any(|root| cwd.starts_with(root) || root.starts_with(cwd))
}

fn path_operand_is_bare_relative(argument: &str) -> bool {
    let normalized = normalize_path_operand(argument);
    !normalized.is_empty()
        && !normalized.starts_with('-')
        && !normalized.starts_with('/')
        && !normalized.starts_with('~')
        && !normalized.starts_with('$')
        && !normalized.contains('/')
        && !normalized.contains(['*', '?', '['])
}

fn cd_target(args: &[String]) -> Option<&str> {
    args.iter()
        .find(|arg| !matches!(arg.as_str(), "-L" | "-P" | "--"))
        .map(String::as_str)
}

fn invocation_recursively_reads_directories(name: &str, args: &[String]) -> bool {
    let has_flag = |long: &str, short_flags: &[char]| {
        args.iter().any(|arg| {
            arg == long
                || (arg.starts_with('-')
                    && !arg.starts_with("--")
                    && arg[1..].chars().any(|flag| short_flags.contains(&flag)))
        })
    };
    match name {
        "rg" | "find" => true,
        "grep" | "egrep" | "fgrep" => {
            has_flag("--recursive", &['r', 'R'])
                || args.iter().enumerate().any(|(index, arg)| {
                    matches!(arg.as_str(), "--directories=recurse" | "-drecurse")
                        || (matches!(arg.as_str(), "-d" | "--directories")
                            && args.get(index + 1).is_some_and(|value| value == "recurse"))
                })
        }
        "cp" => has_flag("--recursive", &['a', 'r', 'R']) || has_flag("--archive", &[]),
        // `--no-recursion` can be another option's value (`--exclude
        // --no-recursion`); tar is treated as recursive.
        "tar" => true,
        "zip" => has_flag("--recurse-paths", &['r', 'R']) || has_flag("--recurse-patterns", &[]),
        "rsync" => has_flag("--archive", &['a']) || has_flag("--recursive", &['r']),
        _ => false,
    }
}

fn recursive_path_operand_indices(command: &str, args: &[String]) -> Vec<usize> {
    let positionals = positional_argument_indices(command, args);
    match command {
        // Whether the first positional is the pattern depends on options
        // (`-ie token`, `--glob -e`); every positional may be an operand.
        "rg" | "grep" | "egrep" | "fgrep" => positionals,
        "find" => args
            .iter()
            .enumerate()
            .take_while(|(_, arg)| !arg.starts_with('-') && !matches!(arg.as_str(), "!" | "("))
            .map(|(index, _)| index)
            .collect(),
        "tar" => tar_input_indices(args),
        "zip" => positionals.into_iter().skip(1).collect(),
        "cp" | "rsync" => {
            let source_count = positionals.len().saturating_sub(1);
            positionals.into_iter().take(source_count).collect()
        }
        _ => Vec::new(),
    }
}

fn positional_argument_indices(command: &str, args: &[String]) -> Vec<usize> {
    let mut indexes = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "--" {
            indexes.extend(index + 1..args.len());
            break;
        }
        if arg.starts_with('-') {
            let takes_value = match command {
                "rg" => matches!(
                    arg,
                    "-e" | "--regexp"
                        | "-g"
                        | "--glob"
                        | "-t"
                        | "--type"
                        | "-T"
                        | "--type-not"
                        | "-m"
                        | "--max-count"
                        | "-A"
                        | "--after-context"
                        | "-B"
                        | "--before-context"
                        | "-C"
                        | "--context"
                        | "-j"
                        | "--threads"
                ),
                "grep" | "egrep" | "fgrep" => matches!(
                    arg,
                    "-e" | "--regexp"
                        | "-f"
                        | "--file"
                        | "-A"
                        | "--after-context"
                        | "-B"
                        | "--before-context"
                        | "-C"
                        | "--context"
                        | "-m"
                        | "--max-count"
                        | "--include"
                        | "--exclude"
                        | "--exclude-dir"
                        | "--binary-files"
                        | "--color"
                        | "--directories"
                ),
                "zip" => matches!(arg, "-i" | "-x" | "-b"),
                "cp" => matches!(
                    arg,
                    "-t" | "--target-directory" | "--sparse" | "--reflink" | "--backup"
                ),
                "rsync" => matches!(
                    arg,
                    "-e" | "--rsh"
                        | "--exclude"
                        | "--include"
                        | "--filter"
                        | "--rsync-path"
                        | "--password-file"
                        | "--backup-dir"
                        | "--suffix"
                ),
                _ => false,
            };
            index += if takes_value { 2 } else { 1 };
            continue;
        }
        indexes.push(index);
        index += 1;
    }
    indexes
}

fn tar_input_indices(args: &[String]) -> Vec<usize> {
    let mut inputs = Vec::new();
    let mut archive_next = false;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        if archive_next {
            archive_next = false;
            index += 1;
            continue;
        }
        // A separate option value is the next word; skip exactly that word.
        if matches!(arg, "--file" | "-f" | "--directory" | "-C") {
            index += 2;
            continue;
        }
        if arg.starts_with("--file=") || arg.starts_with("--") {
            index += 1;
            continue;
        }
        let legacy_flags = arg.trim_start_matches('-');
        if !legacy_flags.is_empty()
            && legacy_flags
                .chars()
                .all(|ch| "cxturAfdOvhzjJZkKpm".contains(ch))
        {
            if legacy_flags.contains('f') {
                archive_next = true;
            }
            index += 1;
            continue;
        }
        if arg.starts_with('-') {
            index += 1;
            continue;
        }
        inputs.push(index);
        index += 1;
    }
    inputs
}

fn recursive_search_defaults_to_cwd(command: &str, args: &[String]) -> bool {
    command == "rg"
        || (matches!(command, "grep" | "egrep" | "fgrep")
            && invocation_recursively_reads_directories(command, args))
}

fn is_script_interpreter(name: &str) -> bool {
    matches!(name, "python" | "python2" | "python3" | "node" | "nodejs")
}

fn shell_argument_is_path(command: &str, argument: &str, cwd: &Path) -> bool {
    if path_operand_is_path_shaped(argument) {
        return true;
    }
    is_file_path_command(command) && store_roots(cwd).iter().any(|root| cwd.starts_with(root))
}

fn is_file_path_command(command: &str) -> bool {
    matches!(
        command,
        "cat"
            | "head"
            | "tail"
            | "less"
            | "more"
            | "cp"
            | "mv"
            | "rsync"
            | "tar"
            | "zip"
            | "base64"
            | "jq"
            | "dd"
            | "od"
            | "xxd"
            | "strings"
            | "sha256sum"
            | "sha1sum"
            | "md5sum"
            | "shasum"
            | "grep"
            | "egrep"
            | "fgrep"
            | "rg"
            | "find"
            | "curl"
            | "wget"
            | "sed"
            | "awk"
            | "source"
    )
}

fn path_operand_is_path_shaped(reference: &str) -> bool {
    let reference = normalize_path_operand(reference);
    !reference.is_empty()
        && (reference.starts_with('/')
            || reference == "~"
            || reference.starts_with("~/")
            || reference.starts_with('$')
            || reference.contains('/')
            || reference.contains(['*', '?', '['])
            || matches!(reference, "." | ".."))
}

fn path_references_store(reference: &str, cwd: &Path, include_parent: bool) -> bool {
    path_references_store_with_glob(reference, cwd, include_parent, true)
}

fn path_references_store_with_glob(
    reference: &str,
    cwd: &Path,
    include_parent: bool,
    match_hidden: bool,
) -> bool {
    let reference = normalize_path_operand(reference);
    if reference.is_empty() {
        return false;
    }
    let roots = store_roots(cwd);
    if let Some(expanded) = expand_path_prefix(reference) {
        let candidate = lexical_absolute(Path::new(&expanded), cwd);
        if candidate.to_string_lossy().contains(['*', '?', '[']) {
            if roots.iter().any(|root| {
                glob_path_may_reach_root(&candidate, root, include_parent, match_hidden)
            }) {
                return true;
            }
        } else if roots.iter().any(|root| {
            candidate.starts_with(root) || (include_parent && root.starts_with(&candidate))
        }) {
            return true;
        }
    }
    alias_references_store(reference, include_parent)
}

fn normalize_path_operand(reference: &str) -> &str {
    let reference = reference.trim_matches(|ch: char| matches!(ch, '\'' | '"' | '`' | ',' | ';'));
    let reference = attached_file_value(reference).unwrap_or(reference);
    let reference = reference
        .split_once('=')
        .filter(|(_, value)| !value.is_empty())
        .map(|(_, value)| value)
        .unwrap_or(reference);
    reference.strip_prefix('@').unwrap_or(reference)
}

/// The value attached to a short-flag cluster such as `-d@x`, `-sSd@x` or
/// `-sT~/x`: everything after the first flag that takes a file value. These
/// are curl's file-reading short flags (data, upload, form, config, header,
/// cookie).
fn attached_file_value(word: &str) -> Option<&str> {
    let flags = word
        .strip_prefix('-')
        .filter(|flags| !flags.starts_with('-'))?;
    let (index, flag) = flags
        .char_indices()
        .take_while(|(_, flag)| flag.is_ascii_alphabetic())
        .find(|(_, flag)| matches!(flag, 'd' | 'T' | 'F' | 'K' | 'H' | 'b'))?;
    Some(&flags[index + flag.len_utf8()..])
}

fn glob_path_may_reach_root(
    pattern: &Path,
    root: &Path,
    include_parent: bool,
    match_hidden: bool,
) -> bool {
    let pattern: Vec<String> = pattern
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let root: Vec<String> = root
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let shared = pattern.len().min(root.len());
    if !pattern[..shared]
        .iter()
        .zip(&root[..shared])
        .all(|(pattern, component)| {
            (match_hidden || !component.starts_with('.') || pattern.starts_with('.'))
                && glob_component_matches(pattern, component)
        })
    {
        return false;
    }
    pattern.len() >= root.len() || include_parent
}

fn glob_component_matches(pattern: &str, value: &str) -> bool {
    fn visit(
        pattern: &[char],
        value: &[char],
        p: usize,
        v: usize,
        memo: &mut std::collections::HashMap<(usize, usize), bool>,
    ) -> bool {
        if let Some(result) = memo.get(&(p, v)) {
            return *result;
        }
        let result = if p == pattern.len() {
            v == value.len()
        } else {
            match pattern[p] {
                '*' => {
                    visit(pattern, value, p + 1, v, memo)
                        || (v < value.len() && visit(pattern, value, p, v + 1, memo))
                }
                '?' => v < value.len() && visit(pattern, value, p + 1, v + 1, memo),
                '[' => {
                    let Some(end) = pattern[p + 1..].iter().position(|ch| *ch == ']') else {
                        return v < value.len()
                            && pattern[p] == value[v]
                            && visit(pattern, value, p + 1, v + 1, memo);
                    };
                    let end = p + 1 + end;
                    v < value.len()
                        && bracket_class_matches(&pattern[p + 1..end], value[v])
                        && visit(pattern, value, end + 1, v + 1, memo)
                }
                ch => {
                    v < value.len() && ch == value[v] && visit(pattern, value, p + 1, v + 1, memo)
                }
            }
        };
        memo.insert((p, v), result);
        result
    }

    let pattern: Vec<char> = pattern.chars().collect();
    let value: Vec<char> = value.chars().collect();
    visit(
        &pattern,
        &value,
        0,
        0,
        &mut std::collections::HashMap::new(),
    )
}

fn bracket_class_matches(class: &[char], value: char) -> bool {
    let (negated, class) = match class.first() {
        Some('!' | '^') => (true, &class[1..]),
        _ => (false, class),
    };
    let mut matched = false;
    let mut index = 0;
    while index < class.len() {
        if index + 2 < class.len() && class[index + 1] == '-' {
            matched |= class[index] <= value && value <= class[index + 2];
            index += 3;
        } else {
            matched |= class[index] == value;
            index += 1;
        }
    }
    matched != negated
}

fn store_roots(cwd: &Path) -> Vec<PathBuf> {
    cas_pty::store_isolation::operator_credential_stores(|name| std::env::var_os(name))
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
    static PYTHON_GLOB_OPERAND: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| {
            regex::Regex::new(r"\bglob\.(?:glob|iglob)\(\s*(?:os\.path\.expanduser\(\s*)?$")
                .expect("static Python glob operand pattern")
        });
    let strings = quoted_strings(script);
    // Python glob's default '*' cannot enter .config or .local. Restrict
    // that allowance to a literal glob operand, never another script path.
    // Explicit or dynamically supplied keyword options stay conservative.
    let default_glob_options = !script.contains("include_hidden") && !script.contains("**");
    if strings.iter().any(|(offset, part)| {
        let match_hidden =
            !default_glob_options || !PYTHON_GLOB_OPERAND.is_match(&script[..*offset]);
        path_operand_is_path_shaped(part)
            && path_references_store_with_glob(part, cwd, false, match_hidden)
    }) {
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
        "expanduser",
        "$home",
        "${home}",
    ]
    .iter()
    .any(|marker| source.contains(marker));
    if !reads_home {
        return false;
    }
    let path_text = source.replace('\\', "/");
    if [
        ".config/neonctl/credentials.json",
        ".local/share/com.vercel.cli/auth.json",
        "library/application support/com.vercel.cli/auth.json",
        ".config/com.vercel.cli/auth.json",
    ]
    .iter()
    .any(|suffix| path_text.contains(suffix))
    {
        return true;
    }
    let components = strings
        .iter()
        .map(|(_, value)| value.as_str())
        .collect::<Vec<_>>()
        .join("/")
        .to_ascii_lowercase();
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

fn quoted_strings(source: &str) -> Vec<(usize, String)> {
    let mut strings = Vec::new();
    let mut chars = source.char_indices().peekable();
    while let Some((offset, ch)) = chars.next() {
        if !matches!(ch, '\'' | '"') {
            continue;
        }
        let quote = ch;
        let mut value = String::new();
        let mut escaped = false;
        for (_, ch) in chars.by_ref() {
            if escaped {
                value.push(ch);
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                strings.push((offset, value));
                break;
            } else {
                value.push(ch);
            }
        }
    }
    strings
}
