//! Resolve recognized shell filesystem effects before applying the workspace
//! policy. This is deliberately a bounded recognizer, never a shell evaluator.
use super::*;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Default)]
pub(super) struct ShellEffects {
    pub writes: Vec<String>,
    pub edits: Vec<String>,
    pub deletes: Vec<(String, bool)>,
    pub links: Vec<(ShellLink, PathBuf)>,
}

type Directories = Vec<Option<PathBuf>>;

pub(super) fn collect(command: &str, cwd: &Path) -> Result<ShellEffects, &'static str> {
    let mut effects = ShellEffects::default();
    collect_script(
        command,
        vec![Some(cwd.to_path_buf())],
        cwd,
        &HashMap::new(),
        0,
        &mut effects,
    )?;
    Ok(effects)
}

fn quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}

fn render(tokens: &[ShellToken]) -> String {
    let mut command = String::new();
    let mut previous_operator = false;
    for token in tokens {
        let operator = matches!(token, ShellToken::Operator(_));
        if !command.is_empty() && !(operator && previous_operator) {
            command.push(' ');
        }
        match token {
            ShellToken::Word(word) => command.push_str(&quote(word)),
            ShellToken::Operator(ch) => command.push(*ch),
        }
        previous_operator = operator;
    }
    command
}

fn rooted(raw: &str, cwd: Option<&Path>, original: &Path) -> Result<String, &'static str> {
    if raw.contains("$(") || raw.contains('`') || raw == "$" {
        return Err("an unresolved shell write target");
    }
    if Path::new(raw).is_absolute() || raw.starts_with('~') {
        return Ok(raw.to_string());
    }
    let cwd = cwd.ok_or("a write with an unresolved effective working directory")?;
    Ok(if cwd == original {
        raw.to_string()
    } else {
        cwd.join(raw).display().to_string()
    })
}

fn add_directory(dirs: &mut Directories, directory: Option<PathBuf>) {
    if !dirs.contains(&directory) {
        dirs.push(directory);
    }
    if dirs.len() > 64 {
        *dirs = vec![None];
    }
}

/// Extract substitutions kept opaque inside a quoted shell word. Unquoted
/// substitutions are already split into subprocess scopes by the tokenizer.
fn substitution_bodies(word: &str) -> Result<Vec<&str>, &'static str> {
    let bytes = word.as_bytes();
    let mut bodies = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes.get(index..index + 2) == Some(b"$(") {
            let end = substitution_end(bytes, index + 2, 0).ok_or("an unresolved shell payload")?;
            bodies.push(&word[index + 2..end]);
            index = end + 1;
        } else if bytes[index] == b'`' {
            let start = index + 1;
            index = start;
            while index < bytes.len() && bytes[index] != b'`' {
                index += if bytes[index] == b'\\' { 2 } else { 1 };
            }
            if index >= bytes.len() {
                return Err("an unresolved shell payload");
            }
            bodies.push(&word[start..index]);
            index += 1;
        } else {
            index += 1;
        }
    }
    Ok(bodies)
}

fn substitution_end(bytes: &[u8], mut index: usize, depth: usize) -> Option<usize> {
    if depth > 8 {
        return None;
    }
    let mut parentheses = 1;
    let mut quote = None;
    while index < bytes.len() {
        let ch = bytes[index];
        if ch == b'\\' && quote != Some(b'\'') {
            index += 2;
            continue;
        }
        if quote == Some(b'\'') {
            if ch == b'\'' {
                quote = None;
            }
        } else if bytes.get(index..index + 2) == Some(b"$(") {
            index = substitution_end(bytes, index + 2, depth + 1)?;
        } else if quote == Some(b'"') {
            if ch == b'"' {
                quote = None;
            }
        } else {
            match ch {
                b'\'' | b'"' => quote = Some(ch),
                b'(' => parentheses += 1,
                b')' => {
                    parentheses -= 1;
                    if parentheses == 0 {
                        return Some(index);
                    }
                }
                _ => {}
            }
        }
        index += 1;
    }
    None
}

fn collect_script(
    script: &str,
    mut dirs: Directories,
    original: &Path,
    inherited: &HashMap<String, Vec<String>>,
    depth: usize,
    effects: &mut ShellEffects,
) -> Result<Directories, &'static str> {
    if depth > 8 {
        return Err("a shell payload beyond the resolution limit");
    }
    let (tokens, literal_words) =
        factory_shell_tokens_with_literals(&shell_command_without_heredoc_bodies(script));
    let mut values = inherited.clone();
    values.extend(factory_shell_variable_values(&tokens));
    // Directory operands use only bindings visible before that cd. The
    // legacy file-target expander above also collects finite loop values.
    let mut directory_values = inherited.clone();
    for name in ["HOME", "CDPATH"] {
        if let Ok(value) = std::env::var(name) {
            directory_values
                .entry(name.into())
                .or_insert_with(|| vec![value]);
        }
    }
    let conditional_bindings = tokens.iter().any(|token| matches!(token,
        ShellToken::Word(word) if matches!(word.as_str(), "if" | "while" | "until" | "for" | "case")));
    let mut alternatives = Vec::new();
    let mut heredocs = shell_heredoc_bodies(script).into_iter();
    let mut scopes = Vec::new();
    let mut start: usize = 0;
    for end in 0..=tokens.len() {
        let separator = match tokens.get(end) {
            Some(ShellToken::Operator(ch @ (';' | '&' | '|' | '(' | ')'))) => Some(*ch),
            None => None,
            _ => continue,
        };
        let assignment_may_skip = conditional_bindings
            || matches!(
                tokens.get(start.wrapping_sub(1)),
                Some(ShellToken::Operator('&' | '|'))
            );
        let mut statement_start = start;
        let mut statement = &tokens[start..end];
        start = end + 1;
        while matches!(statement.first(), Some(ShellToken::Word(word))
            if matches!(word.as_str(), "{" | "if" | "then" | "else" | "elif" | "do" | "while" | "until" | "!"))
        {
            statement = &statement[1..];
            statement_start += 1;
        }
        let incoming = dirs.clone();
        let and_then =
            separator == Some('&') && tokens.get(end + 1) == Some(&ShellToken::Operator('&'));
        if !statement.is_empty() {
            let mut command = render(statement);
            // The existing script-write recognizer needs the body attached to
            // its receiving command, rather than interpreted as shell words.
            for delimiter in heredoc_delimiters(&command) {
                if let Some(body) = heredocs.next() {
                    command.push('\n');
                    command.push_str(&body);
                    command.push_str(&delimiter.value);
                    command.push('\n');
                }
            }
            let words: Vec<String> = statement
                .iter()
                .filter_map(|token| match token {
                    ShellToken::Word(word) => Some(word.clone()),
                    _ => None,
                })
                .collect();
            let word_literals: Vec<bool> = statement
                .iter()
                .enumerate()
                .filter_map(|(index, token)| {
                    matches!(token, ShellToken::Word(_))
                        .then_some(literal_words[statement_start + index])
                })
                .collect();
            let index = unwrapped_command_index(&words);
            let name = index.map(|index| shell_word_basename(&words[index]));
            let args = index.map(|index| &words[index + 1..]).unwrap_or(&[]);
            if name.is_some_and(|name| {
                name.contains(['$', '`', '*', '?'])
                    || (name.contains('[') && !matches!(name, "[" | "[["))
            }) {
                return Err("an unresolved shell executable");
            }

            // A cwd-changing wrapper is scoped to the invoked command. Its
            // computed directory cannot be attributed to the hook's cwd.
            let wrapper_changes_directory = words.iter().take(index.unwrap_or(0)).any(|word| {
                matches!(word.as_str(), "-C" | "--chdir" | "-D" | "--chroot" | "-R")
                    || word.starts_with("--chdir=")
                    || word.starts_with("--chroot=")
            });
            let statement_dirs = if wrapper_changes_directory {
                vec![None]
            } else {
                dirs.clone()
            };

            // A quoted argument may execute a substitution even though the
            // outer tokenizer correctly keeps it as one word. Its filesystem
            // effects are checked in a subprocess scope; its output remains
            // unresolved when used as an executable, directory or write path.
            for (word, literal) in words.iter().zip(&word_literals) {
                if !literal {
                    for body in substitution_bodies(word)? {
                        collect_script(
                            body,
                            statement_dirs.clone(),
                            original,
                            &directory_values,
                            depth + 1,
                            effects,
                        )?;
                    }
                }
            }

            for directory in &statement_dirs {
                let base = directory.as_deref();
                for raw in bash_write_targets(&command) {
                    for raw in expand_factory_shell_word(&raw, &values) {
                        effects.writes.push(rooted(&raw, base, original)?);
                    }
                }
                for raw in bash_in_place_edit_targets(&command) {
                    for raw in expand_factory_shell_word(&raw, &values) {
                        effects.edits.push(rooted(&raw, base, original)?);
                    }
                }
                for (raw, recursive) in bash_delete_targets(&command) {
                    for raw in expand_factory_shell_word(&raw, &values) {
                        effects
                            .deletes
                            .push((rooted(&raw, base, original)?, recursive));
                    }
                }
                if matches!(name, Some("ln" | "link" | "cp")) {
                    let base =
                        base.ok_or("a link with an unresolved effective working directory")?;
                    for link in bash_link_creations(&command, base) {
                        effects.links.push((link, base.to_path_buf()));
                    }
                }
            }

            if name == Some("cd") {
                let mut cdpath = directory_values.get("CDPATH").cloned().unwrap_or_default();
                for word in &words[..index.unwrap_or(0)] {
                    if let Some(value) = word.strip_prefix("CDPATH=") {
                        cdpath = expand_factory_shell_word(value, &directory_values);
                    }
                }
                let searches_directory = cdpath.iter().any(|value| !value.is_empty());
                let operands: Vec<_> = args
                    .iter()
                    .filter(|arg| arg.as_str() != "--" && arg.as_str() != "-L")
                    .collect();
                let targets = match operands.as_slice() {
                    [] => directory_values
                        .get("HOME")
                        .cloned()
                        .unwrap_or_else(|| vec!["$HOME".into()]),
                    [target] if !target.starts_with('-') => {
                        expand_factory_shell_word(target, &directory_values)
                    }
                    _ => vec!["$UNRESOLVED_CWD".into()],
                };
                let mut changed = Vec::new();
                for directory in &dirs {
                    for target in &targets {
                        let path = if target.contains(['$', '`', '*', '?', '['])
                            || (searches_directory
                                && !Path::new(target).is_absolute()
                                && !target.starts_with('~')
                                && !target.starts_with("./")
                                && !target.starts_with("../")
                                && target != "."
                                && target != "..")
                        {
                            // CDPATH changes how a bare relative operand is
                            // searched. Require an explicit ./ or absolute
                            // directory rather than assuming the hook's cwd.

                            None
                        } else if target == "~" || target.starts_with("~/") {
                            std::env::var_os("HOME")
                                .map(PathBuf::from)
                                .map(|home| home.join(target.strip_prefix("~/").unwrap_or("")))
                        } else if Path::new(target).is_absolute() {
                            Some(PathBuf::from(target))
                        } else {
                            directory.as_ref().map(|base| base.join(target))
                        };
                        // Keep both logical and physical interpretations of
                        // parent traversal through a symlink. Neither grants
                        // authority the other would refuse.
                        if let Some(path) = &path {
                            add_directory(
                                &mut changed,
                                Some(lexically_normalize_path(path.clone())),
                            );
                        }
                        add_directory(&mut changed, path);
                    }
                }
                // The next && statement requires cd to have succeeded.
                // Keep failed/skipped alternatives for later ;/|| joins.
                if and_then {
                    dirs = changed;
                } else {
                    for directory in changed {
                        add_directory(&mut dirs, directory);
                    }
                }
            }

            let assignments = if name == Some("export") {
                args
            } else {
                &words[..index.unwrap_or(words.len())]
            };
            for word in assignments {
                if let Some((name, value)) = word.split_once('=')
                    && is_shell_variable_name(name)
                {
                    let expanded = expand_factory_shell_word(value, &directory_values);
                    if assignment_may_skip {
                        let previous = directory_values
                            .entry(name.into())
                            .or_insert_with(|| vec![format!("${name}")]);
                        for value in expanded {
                            if !previous.contains(&value) {
                                previous.push(value);
                            }
                        }
                        if previous.len() > 64 {
                            *previous = vec![format!("${name}")];
                        }
                    } else {
                        directory_values.insert(name.into(), expanded);
                    }
                }
            }

            let mut payload_literal = true;
            let payload = if matches!(
                name,
                Some("sh" | "bash" | "dash" | "zsh" | "ksh" | "script" | "su" | "runuser")
            ) {
                args.iter()
                    .position(|arg| {
                        arg == "--command"
                            || (arg.starts_with('-') && !arg.starts_with("--") && arg.contains('c'))
                    })
                    .map(|position| {
                        payload_literal = index
                            .and_then(|index| word_literals.get(index + position + 2))
                            .copied()
                            .unwrap_or(false);
                        args.get(position + 1)
                            .cloned()
                            .ok_or("a shell command without a resolvable payload")
                    })
                    .transpose()?
            } else if name == Some("eval") {
                payload_literal = word_literals[index.unwrap() + 1..]
                    .iter()
                    .all(|literal| *literal);
                Some(args.join(" "))
            } else {
                let payload = env_split_string_payload(&words);
                if let Some(payload) = &payload {
                    let candidates: Vec<bool> = words
                        .iter()
                        .zip(&word_literals)
                        .filter(|(word, _)| {
                            *word == payload
                                || word.strip_prefix("-S") == Some(payload.as_str())
                                || word.strip_prefix("--split-string=") == Some(payload.as_str())
                        })
                        .map(|(_, literal)| *literal)
                        .collect();
                    payload_literal =
                        !candidates.is_empty() && candidates.iter().all(|literal| *literal);
                }
                payload
            };
            if let Some(payload) = payload {
                if !payload_literal {
                    return Err("an unresolved shell payload");
                }
                let nested = collect_script(
                    &payload,
                    statement_dirs,
                    original,
                    &directory_values,
                    depth + 1,
                    effects,
                )?;
                if name == Some("eval") {
                    for directory in nested {
                        add_directory(&mut dirs, directory);
                    }
                }
            }
        }
        match separator {
            Some('(') => {
                if scopes.len() >= 8 {
                    return Err("a shell payload beyond the resolution limit");
                }
                scopes.push((dirs.clone(), alternatives.clone(), directory_values.clone()))
            }
            Some(')') => {
                let parent = scopes.pop().ok_or("an unmatched shell scope")?;
                dirs = parent.0;
                alternatives = parent.1;
                directory_values = parent.2;
            }
            Some('&') if and_then => {
                for directory in incoming {
                    add_directory(&mut alternatives, directory);
                }
            }
            Some(';') | Some('|') | None => {
                for directory in std::mem::take(&mut alternatives) {
                    add_directory(&mut dirs, directory);
                }
            }
            Some('&') if tokens.get(end.wrapping_sub(1)) != Some(&ShellToken::Operator('&')) => {
                for directory in std::mem::take(&mut alternatives) {
                    add_directory(&mut dirs, directory);
                }
            }
            _ => {}
        }
    }
    if !scopes.is_empty() {
        return Err("an unmatched shell scope");
    }
    Ok(dirs)
}
