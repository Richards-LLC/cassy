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
    collect_script(command, vec![Some(cwd.to_path_buf())], cwd, &HashMap::new(), 0, &mut effects)?;
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
    if Path::new(raw).is_absolute() || raw.starts_with('~') {
        return Ok(raw.to_string());
    }
    let cwd = cwd.ok_or("a write with an unresolved effective working directory")?;
    Ok(if cwd == original { raw.to_string() } else { cwd.join(raw).display().to_string() })
}

fn add_directory(dirs: &mut Directories, directory: Option<PathBuf>) {
    if !dirs.contains(&directory) {
        dirs.push(directory);
    }
    if dirs.len() > 64 {
        *dirs = vec![None];
    }
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
    let tokens = factory_shell_tokens(&shell_command_without_heredoc_bodies(script));
    let mut values = inherited.clone();
    values.extend(factory_shell_variable_values(&tokens));
    let mut heredocs = shell_heredoc_bodies(script).into_iter();
    let mut scopes = Vec::new();
    let mut start = 0;
    for end in 0..=tokens.len() {
        let separator = match tokens.get(end) {
            Some(ShellToken::Operator(ch @ (';' | '&' | '|' | '(' | ')'))) => Some(*ch),
            None => None,
            _ => continue,
        };
        let statement = &tokens[start..end];
        start = end + 1;
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
            let words: Vec<String> = statement.iter().filter_map(|token| match token {
                ShellToken::Word(word) => Some(word.clone()),
                _ => None,
            }).collect();
            let index = unwrapped_command_index(&words);
            let name = index.map(|index| shell_word_basename(&words[index]));
            let args = index.map(|index| &words[index + 1..]).unwrap_or(&[]);

            // A cwd-changing wrapper is scoped to the invoked command. Its
            // computed directory cannot be attributed to the hook's cwd.
            let wrapper_changes_directory = words.iter().take(index.unwrap_or(0)).any(|word| {
                matches!(word.as_str(), "-C" | "--chdir" | "-D" | "--chroot" | "-R")
                    || word.starts_with("--chdir=") || word.starts_with("--chroot=")
            });
            let statement_dirs = if wrapper_changes_directory { vec![None] } else { dirs.clone() };

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
                        effects.deletes.push((rooted(&raw, base, original)?, recursive));
                    }
                }
                if matches!(name, Some("ln" | "link" | "cp")) {
                    let base = base.ok_or("a link with an unresolved effective working directory")?;
                    for link in bash_link_creations(&command, base) {
                        effects.links.push((link, base.to_path_buf()));
                    }
                }
            }

            if name == Some("cd") {
                let operands: Vec<_> = args.iter().filter(|arg| arg.as_str() != "--" && arg.as_str() != "-L").collect();
                let targets = match operands.as_slice() {
                    [] => values.get("HOME").cloned().unwrap_or_else(|| vec!["$HOME".into()]),
                    [target] if !target.starts_with('-') => expand_factory_shell_word(target, &values),
                    _ => vec!["$UNRESOLVED_CWD".into()],
                };
                let mut changed = Vec::new();
                for directory in &dirs {
                    for target in &targets {
                        let path = if target.contains(['$', '`', '*', '?', '[']) {
                            None
                        } else if target == "~" || target.starts_with("~/") {
                            std::env::var_os("HOME").map(PathBuf::from).map(|home| home.join(target.strip_prefix("~/").unwrap_or("")))
                        } else if Path::new(target).is_absolute() {
                            Some(PathBuf::from(target))
                        } else {
                            directory.as_ref().map(|base| base.join(target))
                        };
                        // Keep both logical and physical interpretations of
                        // parent traversal through a symlink. Neither grants
                        // authority the other would refuse.
                        if let Some(path) = &path {
                            add_directory(&mut changed, Some(lexically_normalize_path(path.clone())));
                        }
                        add_directory(&mut changed, path);
                    }
                }
                // `cd` can fail, or be conditional. Retaining the old cwd
                // conservatively covers ;, ||, pipelines and conditionals.
                for directory in changed {
                    add_directory(&mut dirs, directory);
                }
            }

            let payload = if matches!(name, Some("sh" | "bash" | "dash" | "zsh" | "ksh" | "script" | "su" | "runuser")) {
                args.iter().position(|arg| arg == "--command" || (arg.starts_with('-') && !arg.starts_with("--") && arg.contains('c')))
                    .map(|position| args.get(position + 1).cloned().ok_or("a shell command without a resolvable payload"))
                    .transpose()?
            } else if name == Some("eval") {
                Some(args.join(" "))
            } else {
                env_split_string_payload(&words)
            };
            if let Some(payload) = payload {
                if payload.contains(['$', '`']) {
                    return Err("an unresolved shell payload");
                }
                let nested = collect_script(&payload, statement_dirs, original, &values, depth + 1, effects)?;
                if name == Some("eval") {
                    for directory in nested {
                        add_directory(&mut dirs, directory);
                    }
                }
            }
        }
        match separator {
            Some('(') => scopes.push(dirs.clone()),
            Some(')') => dirs = scopes.pop().ok_or("an unmatched shell scope")?,
            _ => {}
        }
    }
    if !scopes.is_empty() {
        return Err("an unmatched shell scope");
    }
    Ok(dirs)
}
