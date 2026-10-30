//! Resolve recognized shell filesystem effects before applying the workspace
//! policy. This is deliberately a bounded recognizer, never a shell evaluator.
use super::*;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Default)]
pub(super) struct ShellEffects {
    /// Effective target and, for relative redirections, its original spelling.
    pub writes: Vec<(String, Option<String>)>,
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
    for (index, token) in tokens.iter().enumerate() {
        let operator = matches!(token, ShellToken::Operator(_));
        // `<<-` is one operator, but the tokenizer leaves its dash on the delimiter
        // word. A space or a quoted dash would make the delimiter `-EOF`.
        let tab_stripping_delimiter = matches!(token, ShellToken::Word(word) if word.starts_with('-'))
            && index >= 2
            && tokens[index - 2..index] == [ShellToken::Operator('<'), ShellToken::Operator('<')];
        if !command.is_empty() && !(operator && previous_operator) && !tab_stripping_delimiter {
            command.push(' ');
        }
        match token {
            ShellToken::Word(word) if tab_stripping_delimiter => {
                command.push('-');
                command.push_str(&quote(&word[1..]));
            }
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

/// Only numeric source is inert under arithmetic's recursive evaluation. Even
/// a bare variable can contain an array subscript that executes a substitution.
fn numeric_arithmetic(expression: &str) -> Result<(), &'static str> {
    if expression.bytes().all(|ch| ch.is_ascii_digit() || ch.is_ascii_whitespace()
        || b"()+-*/%<>=!&|^~?:,".contains(&ch))
    {
        Ok(())
    } else {
        Err("an unresolved arithmetic operand")
    }
}

/// Validate arithmetic commands before their operators become shell scopes.
/// Use source positions: quoted/comment parentheses are ordinary word data.
fn arithmetic_commands(source: &str, lexed: &mut ShellLexed) -> Result<(), &'static str> {
    let mut index = 0;
    while index + 1 < lexed.tokens.len() {
        if lexed.tokens[index..index + 2] != [ShellToken::Operator('('), ShellToken::Operator('(')]
            || lexed.ends[index] + 1 != lexed.ends[index + 1]
        {
            index += 1;
            continue;
        }
        let start = lexed.ends[index + 1];
        let mut balance = 2;
        let mut closing = None;
        for end in index + 2..lexed.tokens.len() {
            match lexed.tokens[end] {
                ShellToken::Operator('(') => balance += 1,
                ShellToken::Operator(')') => {
                    balance -= 1;
                    if balance == 0 {
                        closing = Some(end);
                        break;
                    }
                }
                _ => {}
            }
        }
        let end = closing.ok_or("an unresolved arithmetic operand")?;
        let offset = lexed.ends[end];
        if lexed.tokens[end - 1] != ShellToken::Operator(')')
            || lexed.ends[end - 1] + 1 != offset
        {
            return Err("an unresolved arithmetic operand");
        }
        numeric_arithmetic(&source[start..offset - 2])?;
        // Numeric arithmetic has no filesystem effects. Leave the enclosing
        // command and its redirections in place for the ordinary collector.
        lexed.tokens.splice(index..=end, [ShellToken::Word("true".into())]);
        lexed.literals.splice(index..=end, [true]);
        lexed.ends.splice(index..=end, [offset]);
        index += 1;
    }
    Ok(())
}

/// Numeric comparisons evaluate both operands recursively, even when quoted.
/// Inspect a whole conditional before command separators split its &&/|| arms.
fn conditional_arithmetic(source: &str, lexed: &ShellLexed) -> Result<(), &'static str> {
    let raw = |index: usize| {
        let start = if index == 0 { 0 } else { lexed.ends[index - 1] };
        source[start..lexed.ends[index]].trim()
    };
    let arithmetic = |word: &str| matches!(word, "-eq" | "-ne" | "-lt" | "-gt" | "-le" | "-ge");
    let other_binary = |word: &str| matches!(word, "=" | "==" | "!=" | "=~" | "-nt" | "-ot" | "-ef");
    let mut index = 0;
    while index < lexed.tokens.len() {
        if !matches!(&lexed.tokens[index], ShellToken::Word(word) if word == "[[")
            || raw(index) != "[["
        {
            index += 1;
            continue;
        }
        let start = (0..index).rfind(|&at| matches!(lexed.tokens[at],
            ShellToken::Operator(';' | '&' | '|' | '(' | ')'))).map_or(0, |at| at + 1);
        let mut prefix: Vec<String> = lexed.tokens[start..=index].iter().filter_map(|token| {
            if let ShellToken::Word(word) = token { Some(word.clone()) } else { None }
        }).collect();
        while prefix.first().is_some_and(|word| matches!(word.as_str(),
            "{" | "if" | "then" | "else" | "elif" | "do" | "while" | "until" | "!"))
        {
            prefix.remove(0);
        }
        if !unwrapped_command_index(&prefix).is_some_and(|at| prefix[at] == "[[") {
            index += 1;
            continue;
        }
        let end = (index + 1..lexed.tokens.len()).find(|&at| {
            matches!(&lexed.tokens[at], ShellToken::Word(word) if word == "]]") && raw(at) == "]]"
        }).ok_or("an unresolved conditional operand")?;
        let mut operand = true;
        let mut at = index + 1;
        while at < end {
            // Executable substitutions have their own conditional grammar.
            // The ordinary recursive collector checks their bodies separately.
            if matches!(lexed.tokens[at], ShellToken::Operator('('))
                && at > 0 && matches!(&lexed.tokens[at - 1],
                    ShellToken::Word(word) if word.ends_with('$'))
            {
                let close = substitution_end(source.as_bytes(), lexed.ends[at], 0)
                    .ok_or("an unresolved conditional operand")?;
                at = (at + 1..end).find(|&next| lexed.ends[next] == close + 1)
                    .ok_or("an unresolved conditional operand")?;
                operand = false;
            } else {
                match &lexed.tokens[at] {
                    ShellToken::Operator('&' | '|' | '(') => operand = true,
                    ShellToken::Operator(')') => operand = false,
                    ShellToken::Operator('<' | '>') if !operand => {
                        at += 1;
                        operand = false;
                    }
                    ShellToken::Word(word) => {
                        let binary_follows = at + 2 < end
                            && matches!(&lexed.tokens[at + 1], ShellToken::Word(next)
                                if arithmetic(next) || other_binary(next))
                            && !matches!(lexed.tokens[at + 2], ShellToken::Operator('&' | '|' | ')'));
                        if operand && raw(at) == "!" && !binary_follows {
                            // Conditional negation, not an operand.
                        } else if operand && word.len() == 2 && word.starts_with('-')
                            && !word.as_bytes()[1].is_ascii_digit() && !binary_follows
                        {
                            at += 1; // An ordinary unary test consumes its data operand.
                            operand = false;
                        } else if !operand && arithmetic(word) {
                            let (Some(ShellToken::Word(left)), Some(ShellToken::Word(right))) =
                                (lexed.tokens.get(at - 1), lexed.tokens.get(at + 1).filter(|_| at + 1 < end))
                            else { return Err("an unresolved conditional operand"); };
                            numeric_arithmetic(left)?;
                            numeric_arithmetic(right)?;
                            at += 1;
                            operand = false;
                        } else if !operand && other_binary(word) {
                            at += 1; // String/file comparisons do not reevaluate arithmetic.
                            operand = false;
                        } else {
                            operand = false;
                        }
                    }
                    _ => {}
                }
            }
            at += 1;
        }
        index = end + 1;
    }
    Ok(())
}

/// Parameter syntax comes from the lexer's original source spans, never
/// cooked words that have lost the distinction between data and expansion.
fn parameter_arithmetic(body: &str) -> Result<(), &'static str> {
    if matches!(body, "!" | "#") { return Ok(()); } // PID/count, not indirection.
    if let Some(target) = body.strip_prefix('!') {
        numeric_arithmetic(target)?;
        // Even a numeric parameter name reads an unknown value as syntax.
        return Err("an unresolved indirect parameter");
    }
    let body = body.strip_prefix('#').unwrap_or(body);
    let mut name = body.bytes().take_while(|ch| ch.is_ascii_alphanumeric() || *ch == b'_').count();
    if name == 0 && body.as_bytes().first().is_some_and(|ch| b"@*?$-".contains(ch)) {
        name = 1;
    }
    if name == 0 { return Err("an unresolved parameter operand"); }
    let mut rest = &body[name..];
    while let Some(subscript) = rest.strip_prefix('[') {
        let end = subscript.find(']').ok_or("an unresolved parameter operand")?;
        let index = &subscript[..end];
        if !matches!(index, "@" | "*") { numeric_arithmetic(index)?; }
        rest = &subscript[end + 1..];
    }
    if let Some(position) = rest.strip_prefix(':') {
        // The four default-value operators do not evaluate arithmetic.
        if !position.starts_with(['-', '+', '=', '?']) {
            numeric_arithmetic(position)?;
        }
    } else if !rest.is_empty() && !rest.starts_with(['-', '+', '=', '?', '/', '#', '%']) {
        return Err("an unresolved parameter operand");
    }
    Ok(())
}

/// Extract executable substitutions from a shell word. Arithmetic operands
/// must be numeric source, never values obtained from shell expansion.
fn substitution_bodies(word: &str) -> Result<Vec<&str>, &'static str> {
    let bytes = word.as_bytes();
    let mut bodies = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let bracket = bytes.get(index..index + 2) == Some(b"$[");
        let prefix = if bytes.get(index..index + 3) == Some(b"$((") {
            Some(3)
        } else if bracket {
            Some(2)
        } else {
            None
        };
        if let Some(prefix) = prefix {
            // Arithmetic has different comment/quote rules across shells.
            // Recognize only plain balanced spans, without shell lexing.
            let mut parentheses = 0;
            let mut brackets = 0;
            let mut end = None;
            for (offset, &byte) in bytes.iter().enumerate().skip(index + prefix) {
                match byte {
                    b'#' | b'\n' | b'\r' | b'\'' | b'"' | b'`' | b'\\' => {
                        return Err("an unresolved arithmetic operand");
                    }
                    b'(' => parentheses += 1,
                    b')' if parentheses > 0 => parentheses -= 1,
                    b')' => {
                        if !bracket && bytes.get(offset + 1) == Some(&b')') {
                            end = Some(offset + 1);
                        }
                        break;
                    }
                    b'[' if bracket => brackets += 1,
                    b']' if bracket && brackets > 0 => brackets -= 1,
                    b']' if bracket => {
                        if parentheses == 0 { end = Some(offset); }
                        break;
                    }
                    _ => {}
                }
            }
            if let Some(end) = end {
                let operand_end = if bracket { end } else { end - 1 };
                numeric_arithmetic(&word[index + prefix..operand_end])?;
                index = end + 1;
                continue;
            }
            if bracket {
                return Err("an unresolved arithmetic operand");
            }
            // An unmatched $(( is a command substitution containing a
            // subshell, so retain the ordinary executable-effects path.
        }
        if bytes.get(index..index + 2) == Some(b"$(") {
            let literal_heredoc = literal_cat_heredoc_end(&word[index..]);
            let end = match literal_heredoc {
                Some(end) => index + end,
                None => substitution_end(bytes, index + 2, 0)
                    .ok_or("an unresolved shell payload")?,
            };
            // Other nested heredocs may execute expansions before cat (or an
            // interpreter) sees the body. Never discard those effects as data.
            if literal_heredoc.is_none()
                && word[index + 2..end].lines().any(|line| !heredoc_delimiters(line).is_empty())
            {
                return Err("an unresolved shell payload");
            }
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

/// Heredoc quotes are data, but escaped markers are inert. Remove shell
/// line continuations, then pass only complete, unescaped expansions through
/// the ordinary substitution/arithmetic checks without rewriting their source.
fn heredoc_substitution_bodies(body: &str, delimiter: &HeredocDelimiter) -> Result<Vec<String>, &'static str> {
    let mut logical = String::new();
    let mut chars = body.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' && chars.peek() == Some(&'\n') {
            chars.next();
            continue;
        }
        logical.push(ch);
        if ch == '\\' && let Some(next) = chars.next() {
            logical.push(next);
        }
    }
    // A joined terminator ends the real heredoc before the line-based
    // extractor expects. Its remaining text may be executable shell source.
    if logical.lines().any(|line| {
        let line = line.trim_end_matches('\r');
        let line = if delimiter.strip_leading_tabs { line.trim_start_matches('\t') } else { line };
        line == delimiter.value
    }) {
        return Err("an unresolved shell payload");
    }
    let bytes = logical.as_bytes();
    let mut bodies = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' {
            index += 2;
            continue;
        }
        if bytes.get(index..index + 2) == Some(b"${") {
            let tail = &logical[index + 2..];
            let end = tail.find('}').ok_or("an unresolved parameter operand")?;
            parameter_arithmetic(&tail[..end])?;
            index += 2; // Nested parameter expansions retain their own checks.
            continue;
        }
        let end = if bytes.get(index..index + 2) == Some(b"$(") {
            literal_cat_heredoc_end(&logical[index..])
                .map(|end| index + end)
                .or_else(|| substitution_end(bytes, index + 2, 0))
        } else if bytes.get(index..index + 2) == Some(b"$[") {
            logical[index + 2..].find(']').map(|end| index + 2 + end)
        } else if bytes[index] == b'`' {
            let mut end = index + 1;
            while end < bytes.len() && bytes[end] != b'`' {
                end += if bytes[end] == b'\\' { 2 } else { 1 };
            }
            (end < bytes.len()).then_some(end)
        } else {
            index += 1;
            continue;
        };
        let end = end.ok_or("an unresolved shell payload")?;
        bodies.extend(substitution_bodies(&logical[index..=end])?.into_iter().map(str::to_owned));
        index = end + 1;
    }
    Ok(bodies)
}

/// Closing `)` of exactly `$(cat <<'WORD'\n...\nWORD\n)` (also double
/// quoted delimiters and <<-). No options, redirects or extra commands are
/// accepted. The quoted body cannot expand, so its quotes/parentheses are data.
/// This only finds the boundary; callers still treat the output as dynamic
/// when used as an executable, shell program, directory or write target.
pub(super) fn literal_cat_heredoc_end(word: &str) -> Option<usize> {
    let newline = word.find('\n')?;
    let header = word.get(2..newline)?;
    if !word.starts_with("$(") {
        return None;
    }
    let header = header.trim_matches([' ', '\t']);
    let delimiter = header.strip_prefix("cat")?.trim_start_matches([' ', '\t']);
    let delimiter = delimiter.strip_prefix("<<")?;
    let strip_tabs = delimiter.starts_with('-');
    let delimiter = delimiter.strip_prefix('-').unwrap_or(delimiter).trim_start_matches([' ', '\t']);
    let quote = *delimiter.as_bytes().first()?;
    if !matches!(quote, b'\'' | b'"') {
        return None;
    }
    let delimiter = delimiter.strip_prefix(char::from(quote))?.strip_suffix(char::from(quote))?;
    if delimiter.is_empty() || !delimiter.bytes().all(|ch| ch.is_ascii_alphanumeric() || ch == b'_') {
        return None;
    }
    let mut offset = newline + 1;
    for line in word[offset..].split_inclusive('\n') {
        let candidate = line.strip_suffix('\n').unwrap_or(line);
        let candidate = if strip_tabs { candidate.trim_start_matches('\t') } else { candidate };
        offset += line.len();
        if candidate == delimiter {
            let closing = word[offset..].trim_start_matches([' ', '\t']);
            return closing.starts_with(')').then_some(word.len() - closing.len());
        }
    }
    None
}

/// Resolve a substitution with the ordinary shell lexer. Only real operator
/// tokens can close it: quoted/comment text and case-pattern ')' are data.
pub(super) fn substitution_end(bytes: &[u8], index: usize, depth: usize) -> Option<usize> {
    if depth > 8 { return None; }
    let source = std::str::from_utf8(bytes.get(index..)?).ok()?;
    let projected = shell_command_projection(source, true);
    let lexed = factory_shell_lex(&projected, false, depth);
    enum Phase {
        Word,
        In,
        Pattern { started: bool, parentheses: usize },
        Body,
    }
    struct Case { level: usize, phase: Phase }
    let mut cases: Vec<Case> = Vec::new();
    let mut level = 0;
    let mut arithmetic = vec![false];
    let mut command_start = true;
    let mut previous_operator = None;
    let mut start = 0;
    for (token, &end) in lexed.tokens.iter().zip(&lexed.ends) {
        let raw = source.get(start..end)?.trim();
        start = end;
        let keyword = |word: &str| raw == word;
        let previous = previous_operator;
        previous_operator = match token {
            ShellToken::Operator(ch) => Some((*ch, end)),
            _ => None,
        };
        let mut handled = false;
        let mut close_case = false;
        if let Some(case) = cases.last_mut().filter(|case| case.level == level && !arithmetic[level]) {
            match (&mut case.phase, token) {
                (Phase::Word, ShellToken::Word(_)) => {
                    case.phase = Phase::In;
                    handled = true;
                }
                (Phase::In, ShellToken::Word(word)) => {
                    if word == "in" && keyword(word) {
                        case.phase = Phase::Pattern { started: false, parentheses: 0 };
                    }
                    handled = true;
                }
                (Phase::Pattern { started, .. }, ShellToken::Word(word)) => {
                    close_case = !*started && word == "esac" && keyword(word);
                    *started = true;
                    handled = true;
                }
                (Phase::Pattern { started, parentheses }, ShellToken::Operator('(')) => {
                    if *started { *parentheses += 1; }
                    *started = true;
                    handled = true;
                }
                (Phase::Pattern { parentheses, .. }, ShellToken::Operator(')')) => {
                    if *parentheses > 0 {
                        *parentheses -= 1;
                    } else {
                        case.phase = Phase::Body;
                        command_start = true;
                    }
                    handled = true;
                }
                (Phase::Body, ShellToken::Word(word))
                    if command_start && word == "esac" && keyword(word) => close_case = true,
                (Phase::Body, ShellToken::Operator(';' | '&'))
                    if previous == Some((';', end - 1))
                        && matches!(source.as_bytes().get(end.saturating_sub(2)..end), Some(b";;" | b";&")) =>
                {
                    case.phase = Phase::Pattern { started: false, parentheses: 0 };
                    handled = true;
                }
                _ => {}
            }
        }
        if close_case {
            cases.pop();
            command_start = false;
            continue;
        }
        if handled { continue; }
        match token {
            ShellToken::Word(word) => {
                if command_start && !arithmetic[level] && word == "case" && keyword(word) {
                    cases.push(Case { level, phase: Phase::Word });
                }
                command_start = command_start && keyword(word)
                    && matches!(word.as_str(), "if" | "then" | "else" | "elif" | "while" | "until" | "do" | "!" | "{");
            }
            ShellToken::Operator('(') => {
                let at = end - 1;
                let dollar = at > 0 && source.as_bytes().get(at - 1) == Some(&b'$');
                arithmetic.push(if at == 0 { true } else if dollar {
                    source.as_bytes().get(at + 1) == Some(&b'(')
                } else { arithmetic[level] });
                level += 1;
                command_start = true;
            }
            ShellToken::Operator(')') if level > 0 => {
                arithmetic.pop();
                level -= 1;
                command_start = false;
            }
            ShellToken::Operator(')') => {
                return (cases.is_empty() && lexed.unresolved_at.is_none_or(|at| at >= end))
                    .then_some(index + end - 1);
            }
            ShellToken::Operator(';' | '|' | '&') => command_start = true,
            _ => {}
        }
    }
    None
}

/// What a token means to the enclosing `case`. Code keeps its ordinary handling.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CaseSyntax {
    Code,
    /// Pattern text, which runs nothing.
    Hidden,
    /// The `)` that opens an arm, which is not a subshell's end.
    FirstArm,
    NextArm,
    Esac,
}

/// Classify `case` syntax, so that a pattern's `)` closes no scope and its words
/// run no command. Everything an arm guards stays code. A pattern that can execute
/// something when matched (a substitution, or anything else this cannot read) and
/// an unterminated `case` are refused, like any payload this cannot resolve.
fn case_syntax(source: &str, lexed: &ShellLexed) -> Result<Vec<CaseSyntax>, &'static str> {
    enum Phase {
        Subject,
        In,
        Pattern { started: bool, first: bool },
        Body,
    }
    struct Case {
        level: usize,
        phase: Phase,
    }
    let mut syntax = vec![CaseSyntax::Code; lexed.tokens.len()];
    let mut cases: Vec<Case> = Vec::new();
    let mut level = 0;
    let mut command_start = true;
    let mut previous_operator = None;
    let mut start = 0;
    for (index, (token, &end)) in lexed.tokens.iter().zip(&lexed.ends).enumerate() {
        // A quoted keyword is an ordinary word, as in substitution_end.
        let raw = source.get(start..end).unwrap_or("").trim();
        start = end;
        let previous = previous_operator;
        previous_operator = match token {
            ShellToken::Operator(ch) => Some((*ch, end)),
            _ => None,
        };
        if let Some(case) = cases.last_mut().filter(|case| case.level == level) {
            match (&mut case.phase, token) {
                (Phase::Subject, ShellToken::Word(_)) => {
                    case.phase = Phase::In;
                    continue;
                }
                (Phase::In, ShellToken::Word(word)) => {
                    if word == "in" && raw == word {
                        case.phase = Phase::Pattern { started: false, first: true };
                    }
                    continue;
                }
                (Phase::Pattern { started, first }, ShellToken::Word(word)) => {
                    if !*started && word == "esac" && raw == word {
                        // With no arm there is no entry to restore.
                        syntax[index] = if *first { CaseSyntax::Hidden } else { CaseSyntax::Esac };
                        cases.pop();
                        command_start = false;
                        continue;
                    }
                    if word.contains(['`', '$']) {
                        return Err("an unresolved shell payload");
                    }
                    *started = true;
                    syntax[index] = CaseSyntax::Hidden;
                    continue;
                }
                (Phase::Pattern { started, .. }, ShellToken::Operator('(')) => {
                    if *started {
                        return Err("an unresolved shell payload");
                    }
                    *started = true;
                    syntax[index] = CaseSyntax::Hidden;
                    continue;
                }
                (Phase::Pattern { started: true, .. }, ShellToken::Operator('|')) => {
                    syntax[index] = CaseSyntax::Hidden;
                    continue;
                }
                (Phase::Pattern { started: true, first }, ShellToken::Operator(')')) => {
                    syntax[index] = if *first { CaseSyntax::FirstArm } else { CaseSyntax::NextArm };
                    case.phase = Phase::Body;
                    command_start = true;
                    continue;
                }
                (Phase::Pattern { started: false, .. }, ShellToken::Operator(';' | '&')) => continue,
                (Phase::Pattern { .. }, _) => return Err("an unresolved shell payload"),
                (Phase::Body, ShellToken::Word(word))
                    if command_start && word == "esac" && raw == word =>
                {
                    syntax[index] = CaseSyntax::Esac;
                    cases.pop();
                    command_start = false;
                    continue;
                }
                (Phase::Body, ShellToken::Operator(';' | '&'))
                    if previous == Some((';', end - 1))
                        && matches!(source.as_bytes().get(end.saturating_sub(2)..end), Some(b";;" | b";&")) =>
                {
                    case.phase = Phase::Pattern { started: false, first: false };
                    continue;
                }
                _ => {}
            }
        }
        match token {
            ShellToken::Word(word) => {
                if command_start && word == "case" && raw == word {
                    cases.push(Case { level, phase: Phase::Subject });
                }
                command_start = command_start
                    && raw == word
                    && matches!(word.as_str(), "if" | "then" | "else" | "elif" | "while" | "until" | "do" | "!" | "{");
            }
            ShellToken::Operator('(') => {
                level += 1;
                command_start = true;
            }
            ShellToken::Operator(')') => {
                // A stray close stays for the caller's scope check to refuse.
                level = level.saturating_sub(1);
                command_start = false;
            }
            ShellToken::Operator(';' | '|' | '&') => command_start = true,
            _ => {}
        }
    }
    if cases.is_empty() { Ok(syntax) } else { Err("an unmatched shell scope") }
}

fn pwd_values(dirs: &Directories) -> Vec<String> {
    dirs.iter()
        .map(|dir| {
            dir.as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "$PWD".into())
        })
        .collect()
}

/// Resolve only command-position expansions using earlier finite bindings.
/// Expanded words are operands, not new shell operators; wrappers are then
/// unwrapped again and all known write commands use the existing recognizers.
fn resolve_executable(
    tokens: &mut Vec<ShellToken>,
    literals: &mut Vec<bool>,
    values: &HashMap<String, Vec<String>>,
) -> Result<(), &'static str> {
    for _ in 0..16 {
        let positions: Vec<_> = tokens
            .iter()
            .enumerate()
            .filter_map(|(position, token)| match token {
                ShellToken::Word(word) => Some((position, word.clone())),
                _ => None,
            })
            .collect();
        let words: Vec<_> = positions.iter().map(|(_, word)| word.clone()).collect();
        let Some(index) = unwrapped_command_index(&words) else {
            return Ok(());
        };
        let word = &words[index];
        if !word.contains(['$', '`', '*', '?'])
            && (!word.contains('[') || matches!(word.as_str(), "[" | "[["))
        {
            return Ok(());
        }
        let expanded = expand_factory_shell_word(word, values);
        if expanded.len() != 1 || expanded[0] == *word {
            return Err("an unresolved shell executable");
        }
        let replacement: Vec<_> = expanded[0]
            .split_whitespace()
            .map(|word| ShellToken::Word(word.into()))
            .collect();
        let position = positions[index].0;
        literals.splice(
            position..position + 1,
            std::iter::repeat_n(true, replacement.len()),
        );
        tokens.splice(position..position + 1, replacement);
    }
    Err("an unresolved shell executable")
}

/// The line-based extractor cannot prove Bash boundaries for these forms.
/// Refuse them before projection can hide executable text as heredoc data.
fn check_heredoc_boundaries(script: &str) -> Result<(), &'static str> {
    let joined = script.replace("\\\n", "");
    let projected = shell_command_without_heredoc_bodies(&joined);
    let delimiters: Vec<_> = projected.split('\n').flat_map(heredoc_delimiters).collect();
    if delimiters.iter().any(|delimiter| !delimiter.literal_body)
        && script.chars().any(|ch| ch == '\r' || (!ch.is_ascii() && ch.is_whitespace()))
    {
        return Err("an unresolved shell payload");
    }
    // ANSI-C quoting needs escape decoding and locale quoting can translate
    // the delimiter. Removing a leading dollar alone cannot prove either.
    if !delimiters.is_empty() && projected.split("<<").skip(1).any(|tail| {
        let tail = tail.strip_prefix('-').unwrap_or(tail).trim_start_matches([' ', '\t']);
        tail.starts_with("$'") || tail.starts_with("$\"")
    }) {
        return Err("an unresolved shell payload");
    }
    let mut logical_line = String::new();
    let mut continued = false;
    for line in script.split_inclusive('\n') {
        if let Some(prefix) = line.strip_suffix("\\\n") {
            logical_line.push_str(prefix);
            continued = true;
            continue;
        }
        logical_line.push_str(line);
        if continued && !heredoc_delimiters(&logical_line).is_empty() {
            return Err("an unresolved shell payload");
        }
        logical_line.clear();
        continued = false;
    }
    Ok(())
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
    check_heredoc_boundaries(script)?;
    let projected = shell_command_without_heredoc_bodies(script);
    let mut lexed = factory_shell_lex(&projected, false, 0);
    if lexed.unresolved_at.is_some() {
        return Err("an unresolved shell payload");
    }
    conditional_arithmetic(&projected, &lexed)?;
    for &(start, end) in &lexed.parameters {
        parameter_arithmetic(&projected[start..end])?;
    }
    arithmetic_commands(&projected, &mut lexed)?;
    let case_tokens = case_syntax(&projected, &lexed)?;
    let (tokens, literal_words) = (lexed.tokens, lexed.literals);
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
    directory_values.insert("PWD".into(), pwd_values(&dirs));
    let conditional_bindings = tokens.iter().any(|token| matches!(token,
        ShellToken::Word(word) if matches!(word.as_str(), "if" | "while" | "until" | "for" | "case")));
    let mut alternatives = Vec::new();
    // Rendering quotes every word, including originally unquoted delimiters.
    // Keep the source's expansion provenance paired with the shared extractor.
    let delimiters: Vec<_> = projected.lines().flat_map(heredoc_delimiters).collect();
    let bodies = shell_heredoc_bodies(script);
    if delimiters.len() != bodies.len() {
        return Err("an unresolved shell payload");
    }
    let mut heredocs = delimiters.into_iter().zip(bodies);
    let mut scopes = Vec::new();
    // The directories on entry to each open `case`. Only one arm runs, so no arm
    // sees a `cd` from an earlier one and any of them may be the one that ran.
    let mut case_entries: Vec<Directories> = Vec::new();
    let mut continuation = false;
    let mut start: usize = 0;
    for end in 0..=tokens.len() {
        let syntax = case_tokens.get(end).copied().unwrap_or(CaseSyntax::Code);
        let separator = match (syntax, tokens.get(end)) {
            (CaseSyntax::Esac, _) => {
                for directory in case_entries.pop().unwrap_or_default() {
                    add_directory(&mut dirs, directory);
                }
                continue;
            }
            (CaseSyntax::Hidden, _) => continue,
            (CaseSyntax::FirstArm | CaseSyntax::NextArm, _) => Some(';'),
            (CaseSyntax::Code, Some(ShellToken::Operator(ch @ (';' | '&' | '|' | '(' | ')')))) => Some(*ch),
            (CaseSyntax::Code, None) => None,
            (CaseSyntax::Code, _) => continue,
        };
        let assignment_may_skip = conditional_bindings
            || matches!(
                tokens.get(start.wrapping_sub(1)),
                Some(ShellToken::Operator('&' | '|'))
            );
        // Pattern text and `esac` are not operands of anything.
        let visible: Vec<usize> =
            (start..end).filter(|index| case_tokens[*index] == CaseSyntax::Code).collect();
        let visible_tokens: Vec<ShellToken> = visible.iter().map(|&index| tokens[index].clone()).collect();
        let mut statement = visible_tokens.as_slice();
        let mut skipped_keywords = 0;
        start = end + 1;
        while matches!(statement.first(), Some(ShellToken::Word(word))
            if matches!(word.as_str(), "{" | "if" | "then" | "else" | "elif" | "do" | "while" | "until" | "!"))
        {
            statement = &statement[1..];
            skipped_keywords += 1;
        }
        let substitution_scope = separator == Some('(')
            && matches!(statement.last(), Some(ShellToken::Word(word)) if word.ends_with('$'));
        let incoming = dirs.clone();
        let and_then =
            separator == Some('&') && tokens.get(end + 1) == Some(&ShellToken::Operator('&'));
        if !statement.is_empty() {
            let mut statement = statement.to_vec();
            let mut statement_literals: Vec<bool> =
                visible[skipped_keywords..].iter().map(|&index| literal_words[index]).collect();
            if !continuation {
                resolve_executable(&mut statement, &mut statement_literals, &directory_values)?;
            }
            let mut command = render(&statement);
            if continuation {
                // Words after an argument substitution continue the original
                // command. Redirections still count, but arguments are never
                // reclassified as executables or filesystem commands.
                command = format!("true {command}");
            }
            // The existing script-write recognizer needs the body attached to
            // its receiving command, rather than interpreted as shell words.
            let mut expanding_bodies = Vec::new();
            for expected in heredoc_delimiters(&command) {
                let (delimiter, body) = heredocs.next().ok_or("an unresolved shell payload")?;
                if expected.value != delimiter.value || expected.strip_leading_tabs != delimiter.strip_leading_tabs {
                    return Err("an unresolved shell payload");
                }
                command.push('\n');
                command.push_str(&body);
                command.push_str(&delimiter.value);
                command.push('\n');
                if !delimiter.literal_body {
                    expanding_bodies.extend(heredoc_substitution_bodies(&body, &delimiter)?);
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
                    matches!(token, ShellToken::Word(_)).then_some(statement_literals[index])
                })
                .collect();
            let index = if continuation {
                None
            } else {
                unwrapped_command_index(&words)
            };
            let name = index.map(|index| shell_word_basename(&words[index]));
            let args = index.map(|index| &words[index + 1..]).unwrap_or(&[]);
            if matches!(name, Some("test" | "[")) {
                let args = if name == Some("[") && args.last().is_some_and(|arg| arg == "]") {
                    &args[..args.len() - 1]
                } else { args };
                let mut at = 0;
                while at < args.len() {
                    // Binary operators consume both data operands. In
                    // particular, a string comparison's RHS may be "-eq".
                    let binary = args.get(at + 1).map(String::as_str);
                    if at + 2 < args.len()
                        && matches!(binary, Some("-eq" | "-ne" | "-lt" | "-gt" | "-le" | "-ge"))
                    {
                        numeric_arithmetic(&args[at])?;
                        numeric_arithmetic(args.get(at + 2)
                            .ok_or("an unresolved conditional operand")?)?;
                        at += 3;
                    } else if matches!(binary, Some("=" | "==" | "!=" | "-nt" | "-ot" | "-ef")) {
                        at += 3;
                    } else if matches!(args[at].as_str(), "!" | "(" | ")" | "-a" | "-o") {
                        at += 1;
                    } else if args[at].len() == 2 && args[at].starts_with('-')
                        && !args[at].as_bytes()[1].is_ascii_digit()
                    {
                        at += 2; // A unary predicate consumes one data operand.
                    } else {
                        at += 1;
                    }
                }
            }
            if name == Some("let") {
                for arg in args.iter().filter(|arg| arg.as_str() != "--") {
                    numeric_arithmetic(arg)?;
                }
            }
            // Integer attributes persist into later ordinary assignments.
            // Without modeling those attributes/namerefs, even an initially
            // numeric declaration cannot establish inert future operands.
            if name == Some("integer") || (matches!(name, Some("declare" | "typeset" | "local"))
                && args.iter().any(|arg| arg.contains(['$', '`'])
                    || (arg.starts_with('-') && !arg.contains('=') && arg.contains('i'))))
            {
                return Err("an unresolved arithmetic operand");
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

            for body in expanding_bodies {
                collect_script(
                    &body,
                    statement_dirs.clone(),
                    original,
                    &directory_values,
                    depth + 1,
                    effects,
                )?;
            }

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

            let mut relative_redirections = Vec::new();
            for pair in statement.windows(2) {
                if let [ShellToken::Operator('>'), ShellToken::Word(target)] = pair {
                    for path in expand_factory_shell_word(target, &values) {
                        if !Path::new(&path).is_absolute() && !path.starts_with('~') {
                            relative_redirections.push(path);
                        }
                    }
                }
            }
            for directory in &statement_dirs {
                let base = directory.as_deref();
                for raw in bash_write_targets(&command) {
                    for raw in expand_factory_shell_word(&raw, &values) {
                        let path = rooted(&raw, base, original)?;
                        let redirection = (!Path::new(&path).is_absolute()
                            && relative_redirections.contains(&raw)).then(|| raw.clone());
                        effects.writes.push((path, redirection));
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

            if name == Some("cd") {
                directory_values.insert("PWD".into(), pwd_values(&dirs));
            }
            let assignments = if continuation {
                &words[..0]
            } else if name == Some("export") {
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
                let payload = if continuation {
                    None
                } else {
                    env_split_string_payload(&words)
                };
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
        match syntax {
            CaseSyntax::FirstArm => case_entries.push(dirs.clone()),
            CaseSyntax::NextArm => {
                for directory in case_entries.last().cloned().unwrap_or_default() {
                    add_directory(&mut dirs, directory);
                }
            }
            _ => {}
        }
        match separator {
            Some('(') => {
                if scopes.len() >= 8 {
                    return Err("a shell payload beyond the resolution limit");
                }
                scopes.push((
                    dirs.clone(),
                    alternatives.clone(),
                    directory_values.clone(),
                    substitution_scope,
                ));
                continuation = false;
            }
            Some(')') => {
                let parent = scopes.pop().ok_or("an unmatched shell scope")?;
                dirs = parent.0;
                alternatives = parent.1;
                directory_values = parent.2;
                continuation = parent.3;
            }
            Some('&') if and_then => {
                continuation = false;
                for directory in incoming {
                    add_directory(&mut alternatives, directory);
                }
            }
            Some(';') | Some('|') | None => {
                continuation = false;
                for directory in std::mem::take(&mut alternatives) {
                    add_directory(&mut dirs, directory);
                }
            }
            Some('&') if tokens.get(end.wrapping_sub(1)) != Some(&ShellToken::Operator('&')) => {
                continuation = false;
                for directory in std::mem::take(&mut alternatives) {
                    add_directory(&mut dirs, directory);
                }
            }
            _ => {}
        }
    }
    // Even a quoted body left unclaimed proves that projection/splitting
    // disagreed about shell syntax; it may have hidden subsequent commands.
    if heredocs.next().is_some() {
        return Err("an unresolved shell payload");
    }
    if !scopes.is_empty() {
        return Err("an unmatched shell scope");
    }
    Ok(dirs)
}
