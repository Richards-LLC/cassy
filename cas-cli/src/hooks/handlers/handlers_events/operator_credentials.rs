//! Read-side guard for the operator's Neon and Vercel credential stores.
//!
//! Resolve tool path arguments independently, without opening the target. Shell
//! commands are split into argv words first so nearby whitespace cannot turn
//! unrelated arguments into a path.

use std::path::{Component, Path, PathBuf};

use cas_core::hooks::types::HookInput;
use serde_json::Value;

use cas_pty::store_isolation::HOME_STORE_SUFFIXES as STORE_SUFFIXES;

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

/// GH #2171: the Bash command runs a statement from a directory the guard
/// cannot resolve, so its relative and `$VAR` operands are judged as if they
/// ran inside a credential store.
fn bash_directory_unresolved(input: &HookInput) -> bool {
    let unknown = std::ffi::OsStr::new(super::pre_tool::UNKNOWN_DIRECTORY);
    input.tool_input.as_ref().and_then(|tool_input| tool_input.get("command")).and_then(Value::as_str)
        .and_then(|command| super::pre_tool::shell_read_contexts(command, Path::new(&input.cwd)))
        .is_some_and(|contexts| contexts.iter().flat_map(|context| &context.directories)
            .any(|directory| directory.as_ref().is_none_or(|directory| directory.components().any(|part| part.as_os_str() == unknown))))
}

/// GH #2502, #2473: the command reaches a store only through a directory the
/// guard cannot resolve, so the refusal names that directory, not a store.
pub(super) fn bash_store_reference_needs_unresolved_directory(input: &HookInput) -> bool {
    bash_directory_unresolved(input)
        && input.tool_input.as_ref().and_then(|tool_input| tool_input.get("command")).and_then(Value::as_str)
            .is_some_and(|command| !shell_references_store_with(command, Path::new(&input.cwd), 0, true))
}

fn shell_references_store(command: &str, initial_cwd: &Path, depth: usize) -> bool {
    shell_references_store_with(command, initial_cwd, depth, false)
}

/// A command that cannot be projected is refused only when a word names a store
/// path. An unresolved directory may be any directory, so its statements are
/// judged from every one that could matter, or, with `known_only`, not at all.
fn shell_references_store_with(command: &str, initial_cwd: &Path, depth: usize, known_only: bool) -> bool {
    if depth > 3 { return words_name_store(command, initial_cwd); }
    let Some(contexts) = super::pre_tool::shell_read_contexts(command, initial_cwd) else {
        return words_name_store(command, initial_cwd);
    };
    // Names stay names only if every pipe consumer in the whole command reads
    // just stdin text. Each context below is one statement and cannot see its
    // pipe neighbours, so this is judged here, once, on the whole command.
    let names_inert = depth == 0 && no_substitution(command) && {
        let (arguments, piped) = super::pre_tool::shell_statement_arguments_with_pipes(command);
        piped.iter().zip(&arguments).all(|(piped, argv)| !*piped || consumer_reads_stdin_only(argv))
    };
    for mut context in contexts {
        let parents = context.command.matches("..").count().min(16);
        let unknown = std::ffi::OsStr::new(super::pre_tool::UNKNOWN_DIRECTORY);
        let directories: Vec<PathBuf> = std::mem::take(&mut context.directories).into_iter()
            .filter(|cwd| !known_only || cwd.as_ref().is_some_and(|cwd| !cwd.components().any(|part| part.as_os_str() == unknown)))
            .flat_map(|cwd| cwd_candidates(cwd, parents))
            .collect();
        let statements = super::pre_tool::shell_statement_words(&context.command);
        let arguments = super::pre_tool::shell_statement_arguments(&context.command);
        for (at, words) in statements.iter().enumerate() {
            let Some(index) = super::pre_tool::unwrapped_command_index(words) else { continue; };
            let executable = Path::new(&words[index]).file_name().and_then(|name| name.to_str())
                .unwrap_or(&words[index]).to_ascii_lowercase();
            // Per statement: a listing opens no file.
            let names_only = names_inert && !context.nested && statements.len() == arguments.len()
                && arguments.get(at).is_some_and(|argv| {
                    super::pre_tool::unwrapped_command_index(argv).is_some_and(|other| {
                        argv[other] == words[index] && lists_names_only(&executable, &argv[other + 1..])
                    })
                });
            if directories.iter().any(|cwd| statement_references_store(words, cwd, depth, names_only, &context.values)) {
                return true;
            }
        }
        if super::pre_tool::shell_heredoc_bodies_with_consumer(&context.command).iter().any(|body| {
            directories.iter().any(|cwd| {
                script_constructs_store_path(&body.text, cwd, body.python_source)
                    // A shell executes the body, including any substitution in it.
                    || (body.shell_commands && shell_body_references_store(&body.text, cwd, depth + 1, known_only))
            })
        }) { return true; }
    }
    false
}

fn words_name_store(text: &str, cwd: &Path) -> bool {
    text.split(|c: char| c.is_whitespace() || "\"'`();|&<>=".contains(c))
        .filter(|word| !word.is_empty())
        .any(|word| path_references_store(word, cwd, false))
}

/// A shell executes this heredoc body. Beyond its statements, any word naming a
/// store path counts, wherever it sits (inside a quoted command substitution
/// included): the shell's own parse is not trusted to surface it.
fn shell_body_references_store(body: &str, cwd: &Path, depth: usize, known_only: bool) -> bool {
    body.split(|c: char| c.is_whitespace() || "\"'`()$;|&<>={}".contains(c))
        .filter(|word| !word.is_empty())
        .any(|word| path_references_store(word, cwd, false))
        || shell_references_store_with(body, cwd, depth, known_only)
}

/// The directories that stand for `cwd`. Each unknown entry takes, per store,
/// the store's own component at that depth (or a stand-in below the store),
/// the only spelling under which the directory could reach that store.
fn cwd_candidates(cwd: Option<PathBuf>, parents: usize) -> Vec<PathBuf> {
    let Some(cwd) = cwd else {
        return unresolved_cwd_candidates(parents);
    };
    let unknown = std::ffi::OsStr::new(super::pre_tool::UNKNOWN_DIRECTORY);
    if !cwd.components().any(|part| part.as_os_str() == unknown) {
        return vec![cwd];
    }
    store_roots(Path::new("/"))
        .iter()
        .map(|root| {
            let root: Vec<_> = root.components().collect();
            cwd.components()
                .enumerate()
                .map(|(at, part)| match part.as_os_str() == unknown {
                    true => root.get(at).map_or(std::ffi::OsStr::new("unresolved-cwd"), |part| part.as_os_str()),
                    false => part.as_os_str(),
                })
                .collect()
        })
        .collect()
}

/// Lexically, a relative path from an unknown directory reaches a store only
/// from inside it or from one of its ancestors, after any parent traversal.
/// Each such directory, with up to `parents` stand-in children, covers it.
fn unresolved_cwd_candidates(parents: usize) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    for root in store_roots(Path::new("/")) {
        for ancestor in root.ancestors() {
            let mut candidate = ancestor.to_path_buf();
            for _ in 0..=parents {
                if !candidates.contains(&candidate) {
                    candidates.push(candidate.clone());
                }
                candidate.push("unresolved-cwd");
            }
        }
    }
    candidates
}

fn statement_references_store(
    words: &[String],
    cwd: &Path,
    depth: usize,
    names_only: bool,
    values: &std::collections::HashMap<String, Vec<String>>,
) -> bool {
    if depth > 3 {
        return false;
    }
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
        // The operand's own spelling is always judged. A value the command
        // binds to a variable can only add a refusal, never clear one.
        let mut candidates = vec![arg.clone()];
        for value in super::pre_tool::expand_factory_shell_word(arg, values) {
            if !candidates.contains(&value) {
                candidates.push(value);
            }
        }
        for arg in candidates {
            let explicit_path = shell_argument_is_path(&executable, &arg, cwd);
            let inferred_recursive_path = store_parent_cwd
                && recursive_operands.contains(&index)
                && path_operand_is_bare_relative(&arg);
            if (explicit_path || inferred_recursive_path) && path_references_store(&arg, cwd, recursive)
            {
                return true;
            }
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
            .any(|arg| script_constructs_store_path(arg, cwd, true))
}

/// The directory the command runs in after its wrapper prefix: `env -C DIR`
/// and `sudo -D DIR` (or `--chdir`) move it. `unwrapped_command_index` skips
/// these wrappers, so their directory change is applied here from the
/// skipped words.
pub(super) fn wrapper_cwd(prefix: &[String], cwd: &Path) -> PathBuf {
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

/// Substitutions can turn listed names into file operands, so a command that
/// has one gets no names-only exemption for any statement.
fn no_substitution(command: &str) -> bool {
    !(command.contains("$(") || command.contains('`') || command.contains("<(")
        || command.contains(">("))
}

/// A pipe consumer that only filters its stdin text. Anything that can open
/// the listed names (`xargs`, `sort --files0-from=-`, a `while read` loop) is
/// not one, and keeps the whole command under normal inspection.
fn consumer_reads_stdin_only(argv: &[String]) -> bool {
    let Some(index) = super::pre_tool::unwrapped_command_index(argv) else { return false };
    let name = Path::new(&argv[index]).file_name().and_then(|name| name.to_str())
        .unwrap_or(&argv[index]).to_ascii_lowercase();
    let args = &argv[index + 1..];
    match name.as_str() {
        "head" | "tail" | "wc" | "sort" | "uniq" | "cut" | "tr" => {
            !args.iter().any(|arg| arg.contains("files0"))
        }
        // One pattern, no option that names a file or a directory to walk.
        "rg" | "grep" | "egrep" | "fgrep" => {
            let mut patterns = 0;
            for arg in args {
                if arg.starts_with('-') && !arg.starts_with("--") && arg.len() > 1 {
                    if !arg[1..].chars().all(|flag| "ivcnFEwxoqIasHh".contains(flag)) {
                        return false;
                    }
                } else if arg.starts_with('-') {
                    return false;
                } else {
                    patterns += 1;
                }
            }
            patterns <= 1
        }
        _ => false,
    }
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

/// `literal` is false for a heredoc body the shell expands before the
/// interpreter reads it; only unexpanded text can be judged as source data.
fn script_constructs_store_path(script: &str, cwd: &Path, literal: bool) -> bool {
    if script_passes_file_credential_to_child(script) {
        return true;
    }
    if literal && literal_python_source_edit(script, cwd) {
        return false;
    }
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

/// cas-f3cf: a script that reads a file into an environment's provider
/// credential variable and starts a child with that environment. The file's
/// path does not matter. A bounded recognizer, not general dataflow: string
/// literals never assign, and every scope counts as one, in source order.
fn script_passes_file_credential_to_child(script: &str) -> bool {
    use std::collections::HashSet;
    use tree_sitter::Node;

    /// The provider credentials the worker boundary already withholds.
    fn credential_name(name: &str) -> bool {
        cas_pty::PROTECTED_OPERATOR_ENV.contains(&name)
            || crate::factory_worker_check::ISOLATED_TEST_CREDENTIALS.contains(&name)
    }

    struct Flow<'s> {
        source: &'s str,
        environments: HashSet<&'s str>,
        file_values: HashSet<&'s str>,
    }

    impl<'s> Flow<'s> {
        fn text(&self, node: Node<'_>) -> &'s str {
            &self.source[node.byte_range()]
        }

        fn credential_key(&self, node: Node<'_>) -> bool {
            python_string_literal(node, self.source).is_some_and(credential_name)
        }

        /// File contents, through any method chain such as `.strip()`.
        fn reads_file(&self, node: Node<'_>) -> bool {
            match node.kind() {
                "identifier" => self.file_values.contains(self.text(node)),
                "parenthesized_expression" | "await" => {
                    node.named_child(0).is_some_and(|inner| self.reads_file(inner))
                }
                "call" => node.child_by_field_name("function").is_some_and(|function| {
                    function.kind() == "attribute"
                        && (function.child_by_field_name("attribute").is_some_and(|method| {
                            matches!(self.text(method), "read_text" | "read_bytes" | "read")
                        }) || function
                            .child_by_field_name("object")
                            .is_some_and(|object| self.reads_file(object)))
                }),
                _ => false,
            }
        }

        /// An environment holding a file-read credential.
        fn carries(&self, node: Node<'_>) -> bool {
            let mut cursor = node.walk();
            match node.kind() {
                "identifier" | "attribute" => self.environments.contains(self.text(node)),
                "parenthesized_expression" | "dictionary_splat" | "list_splat" => {
                    node.named_child(0).is_some_and(|inner| self.carries(inner))
                }
                "dictionary" => node.named_children(&mut cursor).any(|part| match part.kind() {
                    "pair" => {
                        part.child_by_field_name("key").is_some_and(|key| self.credential_key(key))
                            && part.child_by_field_name("value").is_some_and(|value| self.reads_file(value))
                    }
                    _ => self.carries(part),
                }),
                "call" => {
                    let Some(arguments) = node.child_by_field_name("arguments") else {
                        return false;
                    };
                    // `env.copy()`, `dict(env)` and `dict(env, KEY=file)`.
                    let copies = node.child_by_field_name("function").is_some_and(|function| {
                        function.kind() == "attribute"
                            && function.child_by_field_name("object").is_some_and(|object| self.carries(object))
                    });
                    copies || self.arguments_carry(arguments)
                }
                _ => false,
            }
        }

        fn arguments_carry(&self, arguments: Node<'_>) -> bool {
            let mut cursor = arguments.walk();
            arguments.named_children(&mut cursor).any(|argument| match argument.kind() {
                "keyword_argument" => {
                    let name = argument.child_by_field_name("name").map(|name| self.text(name));
                    let value = argument.child_by_field_name("value");
                    value.is_some_and(|value| {
                        self.carries(value)
                            || (name.is_some_and(credential_name) && self.reads_file(value))
                    })
                }
                _ => self.carries(argument),
            })
        }

        fn starts_child(&self, function: Node<'_>) -> bool {
            let name = match function.kind() {
                "attribute" => function.child_by_field_name("attribute").map(|name| self.text(name)),
                "identifier" => Some(self.text(function)),
                _ => None,
            };
            name.is_some_and(|name| {
                matches!(
                    name,
                    "run" | "call" | "Popen" | "check_output" | "check_call" | "getoutput"
                        | "getstatusoutput" | "system" | "popen" | "create_subprocess_exec"
                        | "create_subprocess_shell"
                ) || name.starts_with("exec")
                    || name.starts_with("spawn")
                    || name.starts_with("posix_spawn")
            })
        }

        /// Visit in source order; true once a child receives the credential.
        fn visit(&mut self, node: Node<'s>) -> bool {
            if node.kind() == "call" {
                let function = node.child_by_field_name("function");
                let arguments = node.child_by_field_name("arguments");
                if let (Some(function), Some(arguments)) = (function, arguments) {
                    // A child inherits the process environment.
                    if self.starts_child(function)
                        && (self.arguments_carry(arguments)
                            || ["os.environ", "environ"].iter().any(|env| self.environments.contains(env)))
                    {
                        return true;
                    }
                    if function.kind() == "attribute"
                        && function.child_by_field_name("attribute").is_some_and(|method| {
                            matches!(self.text(method), "update" | "setdefault" | "putenv")
                        })
                    {
                        let object = function.child_by_field_name("object").map(|object| self.text(object));
                        let mut cursor = arguments.walk();
                        let parts: Vec<_> = arguments.named_children(&mut cursor).collect();
                        let pair = parts.len() == 2 && self.credential_key(parts[0]) && self.reads_file(parts[1]);
                        if let Some(object) = object.filter(|_| pair || self.arguments_carry(arguments)) {
                            // `os.putenv` sets the process environment.
                            self.environments.insert(if object == "os" { "os.environ" } else { object });
                        }
                    }
                }
            }
            if node.kind() == "assignment"
                && let (Some(left), Some(right)) =
                    (node.child_by_field_name("left"), node.child_by_field_name("right"))
            {
                if self.visit(right) {
                    return true;
                }
                match left.kind() {
                    "subscript" => {
                        let target = left.child_by_field_name("value").map(|value| self.text(value));
                        let key = left.child_by_field_name("subscript");
                        if let Some(target) = target
                            && key.is_some_and(|key| self.credential_key(key))
                            && self.reads_file(right)
                        {
                            self.environments.insert(target);
                        }
                    }
                    "identifier" => {
                        let name = self.text(left);
                        // Rebinding a name discards what it held.
                        self.environments.remove(name);
                        self.file_values.remove(name);
                        if self.carries(right) {
                            self.environments.insert(name);
                        }
                        if self.reads_file(right) {
                            self.file_values.insert(name);
                        }
                    }
                    _ => {}
                }
                return false;
            }
            let mut cursor = node.walk();
            let children: Vec<_> = node.children(&mut cursor).collect();
            children.into_iter().any(|child| self.visit(child))
        }
    }

    // CPython also ends a line at a bare CR; parse the lines it runs.
    let source = script.replace("\r\n", "\n").replace('\r', "\n");
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&tree_sitter_python::LANGUAGE.into()).is_err() {
        return false;
    }
    let Some(tree) = parser.parse(&source, None) else {
        return false;
    };
    Flow { source: &source, environments: HashSet::new(), file_values: HashSet::new() }
        .visit(tree.root_node())
}

/// Plain and raw strings are inert; f-string expressions are executable.
fn python_string_literal<'a>(node: tree_sitter::Node<'_>, source: &'a str) -> Option<&'a str> {
    if node.kind() != "string" {
        return None;
    }
    let spelling = &source[node.byte_range()];
    let quote = spelling.find(['\'', '"'])?;
    if !spelling[..quote]
        .chars()
        .all(|ch| matches!(ch, 'r' | 'R' | 'u' | 'U'))
    {
        return None;
    }
    let spelling = &spelling[quote..];
    let width = if spelling.starts_with("\"\"\"") || spelling.starts_with("'''") {
        3
    } else {
        1
    };
    spelling.get(width..spelling.len().checked_sub(width)?)
}

/// Admit a bounded source-edit program by its actual file targets, rather than
/// scanning replacement text as executable code. Every statement and call must
/// be understood; unknown operations (including nested execution) fall back to
/// the conservative script scan above. Never execute Python to classify it.
fn literal_python_source_edit(source: &str, cwd: &Path) -> bool {
    use std::collections::HashMap;
    use tree_sitter::Node;

    #[derive(Clone, Copy, PartialEq)]
    enum Value {
        Text,
        File,
        Written,
        Predicate,
    }

    fn text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
        &source[node.byte_range()]
    }

    fn value(
        node: Node<'_>,
        source: &str,
        vars: &HashMap<String, Value>,
        cwd: &Path,
        depth: usize,
    ) -> Option<Value> {
        if depth > 32 {
            return None;
        }
        match node.kind() {
            "identifier" => vars.get(text(node, source)).copied(),
            "string" => python_string_literal(node, source).map(|_| Value::Text),
            "parenthesized_expression" => value(node.named_child(0)?, source, vars, cwd, depth + 1),
            "comparison_operator" => {
                let mut cursor = node.walk();
                node.named_children(&mut cursor)
                    .all(|part| value(part, source, vars, cwd, depth + 1) == Some(Value::Text))
                    .then_some(Value::Predicate)
            }
            "call" => {
                let function = node.child_by_field_name("function")?;
                let arguments = node.child_by_field_name("arguments")?;
                let mut cursor = arguments.walk();
                let args: Vec<_> = arguments.named_children(&mut cursor).collect();
                if function.kind() == "identifier"
                    && text(function, source) == "Path"
                    && args.len() == 1
                {
                    let path = python_string_literal(args[0], source)?;
                    // Do not guess Python escapes, dynamic targets or paths
                    // outside the checkout's working directory.
                    return (!path.contains('\\')
                        && lexical_absolute(Path::new(path), cwd).starts_with(cwd)
                        && !path_references_store(path, cwd, false))
                    .then_some(Value::File);
                }
                if function.kind() != "attribute" {
                    return None;
                }
                let receiver = value(
                    function.child_by_field_name("object")?,
                    source,
                    vars,
                    cwd,
                    depth + 1,
                )?;
                let method = text(function.child_by_field_name("attribute")?, source);
                match (receiver, method, args.len()) {
                    (Value::File, "read_text", 0) => Some(Value::Text),
                    (Value::File, "write_text", 1)
                        if value(args[0], source, vars, cwd, depth + 1) == Some(Value::Text) =>
                    {
                        Some(Value::Written)
                    }
                    (Value::Text, "replace", 2)
                        if args.iter().all(|arg| {
                            value(*arg, source, vars, cwd, depth + 1) == Some(Value::Text)
                        }) =>
                    {
                        Some(Value::Text)
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    if !source.contains("from pathlib import Path") || !source.contains("write_text") {
        return false;
    }
    // The allow path is sound only if tree-sitter sees the statements CPython
    // runs. CPython also ends a line at a bare CR, and a PEP 263 coding cookie
    // (e.g. utf-7) can decode comment bytes into new lines. Only printable
    // ASCII, '\n' and '\t' without a cookie read the same to both.
    if !source.bytes().all(|byte| matches!(byte, b'\n' | b'\t' | b' '..=b'~'))
        || source.lines().take(2).any(|line| {
            line.trim_start().starts_with('#')
                && (line.contains("coding:") || line.contains("coding="))
        })
    {
        return false;
    }
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .is_err()
    {
        return false;
    }
    let Some(tree) = parser.parse(source, None) else {
        return false;
    };
    let root = tree.root_node();
    if root.has_error() {
        return false;
    }
    let mut imported = false;
    let mut writes = false;
    let mut vars = HashMap::new();
    let mut cursor = root.walk();
    for statement in root.named_children(&mut cursor) {
        match statement.kind() {
            "comment" => {}
            "import_from_statement" if text(statement, source) == "from pathlib import Path" => {
                imported = true
            }
            "assert_statement"
                if imported
                    && statement.named_child_count() == 1
                    && statement
                        .named_child(0)
                        .and_then(|node| value(node, source, &vars, cwd, 0))
                        == Some(Value::Predicate) => {}
            "expression_statement" if imported && statement.named_child_count() == 1 => {
                let Some(expression) = statement.named_child(0) else {
                    return false;
                };
                let result = if expression.kind() == "assignment" {
                    let Some(left) = expression.child_by_field_name("left") else {
                        return false;
                    };
                    let Some(right) = expression.child_by_field_name("right") else {
                        return false;
                    };
                    if left.kind() != "identifier"
                        || text(left, source) == "Path"
                        || expression.child_by_field_name("type").is_some()
                    {
                        return false;
                    }
                    let Some(result) = value(right, source, &vars, cwd, 0) else {
                        return false;
                    };
                    vars.insert(text(left, source).to_string(), result);
                    result
                } else {
                    let Some(result) = value(expression, source, &vars, cwd, 0) else {
                        return false;
                    };
                    result
                };
                writes |= result == Value::Written;
            }
            _ => return false,
        }
    }
    imported && writes
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

#[cfg(test)]
mod source_edit_tests {
    use super::literal_python_source_edit;

    /// cas-5d392: tree-sitter's comment token runs to '\n', while CPython also
    /// ends a line at a bare '\r'. A script whose lines the two split
    /// differently is never a recognised literal source edit.
    #[test]
    fn bare_carriage_return_is_not_a_literal_source_edit_cas_5d392() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("fixture.rs"), "old\n").unwrap();
        let edit = |comment: &str| format!(
            "from pathlib import Path\np = Path('fixture.rs')\ns = p.read_text(){comment}\np.write_text(s.replace('old', 'new'))"
        );
        assert!(literal_python_source_edit(&edit(""), dir.path()), "control edit is admitted");
        assert!(!literal_python_source_edit(&edit("\n#x\rs = s"), dir.path()));
    }

    /// cas-5d392: a PEP 263 coding cookie lets CPython decode comment bytes
    /// into statements tree-sitter never parses.
    #[test]
    fn coding_cookie_is_not_a_literal_source_edit_cas_5d392() {
        let dir = tempfile::tempdir().unwrap();
        let edit = "from pathlib import Path\np = Path('fixture.rs')\ns = p.read_text()\np.write_text(s.replace('old', 'new'))";
        assert!(literal_python_source_edit(edit, dir.path()), "control edit is admitted");
        assert!(!literal_python_source_edit(&format!("# coding: utf-7\n{edit}"), dir.path()));
    }

    /// cas-5d392: the shell expands an unquoted heredoc body before Python
    /// reads it, so only a quoted delimiter's body is judged as source data.
    #[test]
    fn expanded_heredoc_body_is_not_a_literal_source_edit_cas_5d392() {
        let dir = tempfile::tempdir().unwrap();
        let command = |delimiter: &str| format!(
            "python3 - <<{delimiter}\nfrom pathlib import Path\np = Path('fixture.rs')\ns = p.read_text()\np.write_text(s.replace('old', '~/.config/neonctl/credentials.json'))\nPY"
        );
        assert!(!super::shell_references_store(&command("'PY'"), dir.path(), 0), "quoted body is data");
        assert!(super::shell_references_store(&command("PY"), dir.path(), 0));
    }
}
