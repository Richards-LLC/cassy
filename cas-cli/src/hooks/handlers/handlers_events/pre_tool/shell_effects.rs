//! Resolve recognized shell filesystem effects before applying the workspace
//! policy. This is deliberately a bounded recognizer, never a shell evaluator.
use super::*;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

#[derive(Default)]
pub(super) struct ShellEffects {
    /// Effective target and, for relative redirections, its original spelling.
    pub writes: Vec<(String, Option<String>)>,
    pub directory_sinks: HashSet<String>,
    pub edits: Vec<String>,
    pub deletes: Vec<(String, bool)>,
    pub links: Vec<(ShellLink, PathBuf)>,
    read_contexts: Option<Vec<ShellReadContext>>,
    /// The read projection saw a function or coprocess body.
    deferred_bodies: bool,
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

/// Opt-in read projection; `None` when the command cannot be projected.
/// Collecting workspace write effects keeps its current behavior.
pub(super) fn read_contexts(command: &str, cwd: &Path) -> Option<Vec<ShellReadContext>> {
    let mut effects = ShellEffects { read_contexts: Some(Vec::new()), ..Default::default() };
    collect_script(command, vec![Some(cwd.to_path_buf())], cwd, &HashMap::new(), 0, &mut effects).ok()?;
    let mut contexts = effects.read_contexts?;
    // A function body runs wherever it is later called, so every statement
    // is judged from every directory the command can reach.
    if effects.deferred_bodies {
        let mut reachable = Vec::new();
        for directory in contexts.iter().flat_map(|context| &context.directories) {
            if !reachable.contains(directory) {
                reachable.push(directory.clone());
            }
        }
        for context in &mut contexts {
            context.directories = reachable.clone();
        }
    }
    Some(contexts)
}

/// A pipe or background `&` at `at`: not `||`, `&&`, the `&` of `|&`, or
/// part of a redirection such as `>|`, `>&` or `&>`.
fn lone_operator(tokens: &[ShellToken], source: &str, ends: &[usize], at: usize) -> Option<char> {
    let ShellToken::Operator(operator @ ('|' | '&')) = tokens.get(at)? else {
        return None;
    };
    let before = at.checked_sub(1).and_then(|index| tokens.get(index));
    let joins = |token: Option<&ShellToken>| matches!(token, Some(ShellToken::Operator(other)) if other == operator);
    let redirect = matches!(before, Some(ShellToken::Operator('<' | '>')))
        || (*operator == '&'
            && (matches!(before, Some(ShellToken::Operator('|')))
                || source.get(*ends.get(at)?..).is_some_and(|rest| rest.starts_with('>'))));
    (!joins(before) && !joins(tokens.get(at + 1)) && !redirect).then_some(*operator)
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

thread_local! {
    /// Names [`numeric_locals`] proved numeric in the script being resolved.
    static NUMERIC_LOCALS: std::cell::RefCell<HashSet<String>> = Default::default();
}

/// Scopes [`NUMERIC_LOCALS`] to one script; a nested payload restores its parent's.
struct NumericLocalsScope(HashSet<String>);

impl NumericLocalsScope {
    fn enter(names: HashSet<String>) -> Self {
        Self(NUMERIC_LOCALS.with(|locals| locals.replace(names)))
    }
}

impl Drop for NumericLocalsScope {
    fn drop(&mut self) {
        NUMERIC_LOCALS.with(|locals| *locals.borrow_mut() = std::mem::take(&mut self.0));
    }
}

/// Only numeric source is inert under arithmetic's recursive evaluation. Even
/// a bare variable can contain an array subscript that executes a substitution,
/// so an operand name is accepted only when the script itself keeps it numeric
/// ([`numeric_locals`], GH #1866).
fn numeric_arithmetic(expression: &str) -> Result<(), &'static str> {
    let bytes = expression.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let ch = bytes[index];
        if ch.is_ascii_digit() || ch.is_ascii_whitespace() || b"()+-*/%<>=!&|^~?:,".contains(&ch) {
            index += 1;
            continue;
        }
        let (start, braced) = match (ch, bytes.get(index + 1)) {
            (b'$', Some(b'{')) => (index + 2, true),
            (b'$', _) => (index + 1, false),
            _ => (index, false),
        };
        let end = start + bytes[start..].iter().take_while(|ch| ch.is_ascii_alphanumeric() || **ch == b'_').count();
        let name = &expression[start..end];
        let closed = !braced || bytes.get(end) == Some(&b'}');
        if !closed || !is_shell_variable_name(name) || !NUMERIC_LOCALS.with(|locals| locals.borrow().contains(name)) {
            return Err("an unresolved arithmetic operand");
        }
        index = end + usize::from(braced);
    }
    Ok(())
}

/// Where a byte of a script sits, for [`numeric_locals`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Lexeme {
    /// Shell code, including `$(...)` opened inside double quotes.
    Code,
    /// Text inside double quotes, where only expansions are live.
    Double,
    /// Inert text: single-quoted text, escaped characters and quote marks.
    Literal,
}

/// Classify every byte of `source` (heredoc bodies already projected out) by
/// Bash's quoting rules: a backslash quotes the next character (manual
/// 3.1.2.1), single quotes preserve every character up to the next single
/// quote (3.1.2.2), and double quotes keep text literal except `$`, backtick
/// and backslash (3.1.2.3), where `$(` opens code again. `None` for a script
/// using what this map does not model, so no counter is proved: a comment
/// (3.1.3), ANSI-C `$'...'` quoting (3.1.2.4) or a backslash-newline line
/// continuation, which Bash removes before splitting words.
fn lexeme_map(source: &str) -> Option<Vec<Lexeme>> {
    let bytes = source.as_bytes();
    if source.contains("\\\n") || source.contains("$'") {
        return None;
    }
    let mut map = vec![Lexeme::Literal; bytes.len()];
    // Open contexts: `true` is a double-quoted string, `false` a parenthesis
    // of code nested inside one.
    let mut stack: Vec<bool> = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let ch = bytes[index];
        if stack.last() == Some(&true) {
            match ch {
                b'\\' => index += 2,
                b'"' => {
                    stack.pop();
                    index += 1;
                }
                b'$' if bytes.get(index + 1) == Some(&b'(') => {
                    map[index] = Lexeme::Code;
                    map[index + 1] = Lexeme::Code;
                    stack.push(false);
                    index += 2;
                }
                _ => {
                    map[index] = Lexeme::Double;
                    index += 1;
                }
            }
            continue;
        }
        match ch {
            b'\\' => index += 2,
            b'\'' => index = source[index + 1..].find('\'').map_or(bytes.len(), |end| index + end + 2),
            b'"' => {
                stack.push(true);
                index += 1;
            }
            // Any `#` that could start a comment (a word's first character).
            b'#' if index == 0 || !bytes[index - 1].is_ascii_alphanumeric() && !b"$_{".contains(&bytes[index - 1]) => {
                return None;
            }
            _ => {
                map[index] = Lexeme::Code;
                if ch == b'(' && !stack.is_empty() {
                    stack.push(false);
                } else if ch == b')' && stack.last() == Some(&false) {
                    stack.pop();
                }
                index += 1;
            }
        }
    }
    Some(map)
}

/// Shell-local names a script keeps numeric, so arithmetic over them cannot
/// evaluate injected source (GH #1866). A name qualifies only when its first
/// occurrence is a literal-integer assignment that runs unconditionally (no
/// control construct, pipe, group or substitution before it), and every later
/// occurrence is another assignment of an integer or of `$((...))`, a plain
/// `$name`/`${name}` read, or an operand inside an arithmetic span. Any other
/// use (`read`, `for name in`, `declare`, `${name:=...}`, quoted text, ...)
/// disqualifies it.
fn numeric_locals(source: &str) -> HashSet<String> {
    // A builtin can assign a name spelled through quoting (`printf -v "mis"sing`)
    // or a computed command can be one; then no textual proof holds.
    let lexed = factory_shell_lex(source, false, 0);
    if lexed.unresolved_at.is_some() {
        return HashSet::new();
    }
    let assigning = shell_statements_from_tokens(lexed.tokens).iter().any(|words| {
        let mut words = words.iter().map(String::as_str).skip_while(|word| {
            matches!(*word, "!" | "{" | "}" | "if" | "then" | "elif" | "else" | "fi" | "do" | "done"
                | "while" | "until" | "for" | "select" | "case" | "esac" | "in" | "time" | "function")
                || word.split_once('=').is_some_and(|(name, _)| is_shell_variable_name(name))
        });
        words.next().is_some_and(|command| {
            !command.bytes().all(|ch| ch.is_ascii_alphanumeric() || b"_./+-=:,%@[]".contains(&ch))
                || matches!(shell_word_basename(command), "eval" | "read" | "printf" | "declare" | "typeset"
                    | "local" | "export" | "readonly" | "mapfile" | "readarray" | "getopts" | "source" | "."
                    | "trap" | "command" | "builtin" | "exec" | "env" | "alias" | "enable" | "wait" | "fc")
        })
    });
    if assigning {
        return HashSet::new();
    }
    let bytes = source.as_bytes();
    let ident = |ch: u8| ch.is_ascii_alphanumeric() || ch == b'_';
    let Some(lexemes) = lexeme_map(source) else {
        return HashSet::new();
    };
    let code = |position: usize| lexemes.get(position) == Some(&Lexeme::Code);
    // Spans inside `((...))` and `$((...))`, which the arithmetic checks validate.
    // Quoted, escaped or commented parens are text, never part of a span.
    let mut arithmetic = Vec::new();
    let mut at = 0;
    while let Some(found) = source[at..].find("((") {
        let open = at + found;
        if !code(open) || !code(open + 1) {
            at = open + 1;
            continue;
        }
        let mut depth = 0usize;
        let mut close = None;
        for (offset, &ch) in bytes.iter().enumerate().skip(open) {
            if !code(offset) {
                continue;
            }
            match ch {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(offset);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(close) = close else { break };
        // A span holding anything but arithmetic (quotes, a statement break,
        // a substitution) is not trusted to keep a counter numeric.
        if !source[open..=close].bytes().all(|ch| ident(ch) || ch == b' ' || ch == b'\t'
            || b"()+-*/%<>=!&|^~?:,${}".contains(&ch))
        {
            at = open + 1;
            continue;
        }
        arithmetic.push(open..close);
        at = close;
    }
    let in_arithmetic = |position: usize| arithmetic.iter().any(|span| span.contains(&position));
    // The value of an assignment starting at `value`: an integer or one `$((...))`.
    let numeric_value = |value: usize| -> Option<usize> {
        let digits = bytes[value..].iter().take_while(|ch| ch.is_ascii_digit()).count();
        if digits > 0 {
            return Some(value + digits);
        }
        let span = arithmetic.iter().find(|span| span.start == value + 1 && bytes.get(value) == Some(&b'$'))?;
        Some(span.end + 1)
    };
    let ends_word = |position: usize| bytes.get(position).is_none_or(|ch| b" \t\n;&|)".contains(ch));
    let unconditional = |prefix: &str| {
        !prefix.contains(['|', '&', '(', '{', '`', '$', '<', '>'])
            && !prefix.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_').any(|word| matches!(word,
                "if" | "then" | "elif" | "else" | "while" | "until" | "for" | "do" | "case" | "select" | "function"))
    };
    let mut candidates: Vec<&str> = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if ident(bytes[index]) && (index == 0 || !ident(bytes[index - 1])) {
            let end = index + bytes[index..].iter().take_while(|ch| ident(**ch)).count();
            let name = &source[index..end];
            if bytes.get(end) == Some(&b'=') && is_shell_variable_name(name) && !candidates.contains(&name) {
                candidates.push(name);
            }
            index = end;
        } else {
            index += 1;
        }
    }
    // Arithmetic source that may only be digits, operators and names; the
    // names must themselves be numeric locals (checked once all are known).
    let arithmetic_source = |text: &str| -> Option<String> {
        text.bytes().all(|ch| ident(ch) || ch.is_ascii_whitespace() || b"()+-*/%<>=!&|^~?:,${}".contains(&ch))
            .then(|| text.to_string())
    };
    // An assignment operator after a name inside arithmetic: its end, if any.
    let arithmetic_assignment = |after: usize| -> Option<usize> {
        let at = after + bytes[after..].iter().take_while(|ch| ch.is_ascii_whitespace()).count();
        let rest = &bytes[at..];
        if rest.starts_with(b"<<=") || rest.starts_with(b">>=") {
            Some(at + 3)
        } else if rest.first() == Some(&b'=') && rest.get(1) != Some(&b'=') {
            Some(at + 1)
        } else if rest.first().is_some_and(|ch| b"+-*/%&^|".contains(ch)) && rest.get(1) == Some(&b'=') {
            Some(at + 2)
        } else {
            None
        }
    };
    let mut names = HashSet::new();
    let mut sources: Vec<(&str, String)> = Vec::new();
    for name in candidates {
        let mut name_sources = Vec::new();
        let mut first = true;
        let mut keeps_numeric = true;
        let mut from = 0;
        while let Some(found) = source[from..].find(name) {
            let start = from + found;
            let end = start + name.len();
            from = end;
            if (start > 0 && ident(bytes[start - 1])) || bytes.get(end).is_some_and(|ch| ident(*ch)) {
                continue;
            }
            let before = &source[..start];
            let assignment_head = before.is_empty()
                || before.ends_with([' ', '\t', '\n', ';', '&', '|', '(']);
            let assigns = bytes.get(end) == Some(&b'=') && bytes.get(end + 1) != Some(&b'=') && assignment_head;
            // Outside shell code only a double-quoted `$name`/`${name}` read is inert.
            let use_ok = if !code(start) {
                !first && lexemes[start] == Lexeme::Double
                    && (before.ends_with('$') || (before.ends_with("${") && bytes.get(end) == Some(&b'}')))
            } else if first {
                assigns && unconditional(before) && bytes[end + 1..].first().is_some_and(u8::is_ascii_digit)
                    && numeric_value(end + 1).is_some_and(ends_word)
            } else if let Some(span) = arithmetic.iter().find(|span| span.contains(&start)) {
                // An assignment inside arithmetic keeps the name numeric only
                // when its value is arithmetic over numeric locals.
                match arithmetic_assignment(end) {
                    Some(value) => arithmetic_source(&source[value.min(span.end)..span.end])
                        .map(|text| name_sources.push(text)).is_some(),
                    None => true,
                }
            } else if assigns {
                numeric_value(end + 1).is_some_and(ends_word)
                    && arithmetic.iter().find(|span| span.start == end + 2)
                        .is_none_or(|span| arithmetic_source(&source[span.start + 2..span.end])
                            .map(|text| name_sources.push(text)).is_some())
            } else if before.ends_with('$') {
                true
            } else if before.ends_with("${") {
                bytes.get(end) == Some(&b'}')
            } else {
                in_arithmetic(start)
            };
            first = false;
            if !use_ok {
                keeps_numeric = false;
                break;
            }
        }
        if keeps_numeric && !first {
            names.insert(name.to_string());
            sources.extend(name_sources.into_iter().map(|text| (name, text)));
        }
    }
    // Drop a name assigned from any name that is not itself kept numeric,
    // until no assignment depends on a dropped one.
    loop {
        let unproven: Vec<&str> = sources.iter().filter(|(name, text)| names.contains(*name)
            && text.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
                .any(|word| word.starts_with(|ch: char| ch.is_ascii_alphabetic() || ch == '_') && !names.contains(word)))
            .map(|(name, _)| *name).collect();
        if unproven.is_empty() {
            return names;
        }
        for name in unproven {
            names.remove(name);
        }
    }
}

/// `test` receives expanded arguments as data; a captured shell exit status is
/// a known integer even though the token is a variable reference. Keep other
/// unknown operands conservative so `[[ ]]` and arithmetic evaluation still
/// fail closed.
fn numeric_test_operand(
    operand: &str,
    captured_status: &HashSet<String>,
) -> Result<(), &'static str> {
    if operand == "$?"
        || operand
            .strip_prefix('$')
            .is_some_and(|name| captured_status.contains(name))
    {
        Ok(())
    } else {
        numeric_arithmetic(operand)
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
        // Any raw unquoted conditional is checked, including function and
        // coprocess bodies whose command prefixes this recognizer cannot prove.
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
fn substitution_bodies(word: &str) -> Result<Vec<String>, &'static str> {
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
            bodies.push(word[index + 2..end].to_owned());
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
            bodies.push(backquote_body(&word[start..index]));
            index += 1;
        } else {
            index += 1;
        }
    }
    Ok(bodies)
}

/// Backquotes remove one escape layer before parsing their command, even
/// inside its quotes. Other backslashes must survive for the inner parser.
pub(super) fn backquote_body(source: &str) -> String {
    let mut body = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' && chars.peek().is_some_and(|next| matches!(next, '`' | '\\' | '$')) {
            body.push(chars.next().unwrap());
        } else {
            body.push(ch);
        }
    }
    body
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
        bodies.extend(substitution_bodies(&logical[index..=end])?);
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

/// Marks each token that belongs to a redirect rather than the command: its
/// filename, and a numbered or named descriptor in front of it. Bash runs `>out touch /x` as
/// `touch /x`, so neither may be mistaken for the executable or an operand.
fn redirect_words(tokens: &[ShellToken]) -> Vec<bool> {
    let redirect = |token: Option<&ShellToken>| matches!(token, Some(ShellToken::Operator('<' | '>')));
    let mut marked = vec![false; tokens.len()];
    let mut index = 0;
    while index < tokens.len() {
        if !redirect(tokens.get(index)) {
            if let ShellToken::Word(word) = &tokens[index]
                && is_redirect_descriptor(word, tokens.get(index + 1))
            {
                marked[index] = true;
            }
            index += 1;
            continue;
        }
        while matches!(tokens.get(index), Some(ShellToken::Operator('<' | '>' | '&' | '|'))) {
            index += 1;
        }
        if matches!(tokens.get(index), Some(ShellToken::Word(_))) {
            marked[index] = true;
            index += 1;
        }
    }
    marked
}

/// Whether this operator continues a redirect, as in `>&` and `>|`. The next word
/// is that redirect's filename, so it is not in command position and cannot be a
/// keyword. Bash runs `>case touch in /outside/p` as `touch in /outside/p`.
fn after_redirect(previous: Option<(char, usize)>, end: usize) -> bool {
    matches!(previous, Some(('<' | '>', at)) if at + 1 == end)
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
            ShellToken::Operator('<' | '>') => command_start = false,
            ShellToken::Operator(';' | '|' | '&') => command_start = !after_redirect(previous, end),
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
                    // Words in a pattern are joined by `|`. Two in a row are a command
                    // this has misread, and must never be hidden as pattern text.
                    let after_word = index > 0
                        && syntax[index - 1] == CaseSyntax::Hidden
                        && matches!(lexed.tokens[index - 1], ShellToken::Word(_));
                    if after_word || word.contains(['`', '$']) {
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
            ShellToken::Operator('<' | '>') => command_start = false,
            ShellToken::Operator(';' | '|' | '&') => command_start = !after_redirect(previous, end),
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
        let redirects = redirect_words(tokens);
        let positions: Vec<_> = tokens
            .iter()
            .enumerate()
            .filter_map(|(position, token)| match token {
                ShellToken::Word(word) if !redirects[position] => Some((position, word.clone())),
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
    if !delimiters.is_empty()
        && script.chars().any(|ch| ch.is_whitespace() && !matches!(ch, ' ' | '\t' | '\n'))
    {
        return Err("an unresolved shell payload");
    }
    // ANSI-C quoting needs escape decoding and locale quoting can translate
    // the delimiter. Removing a leading dollar alone cannot prove either.
    if !delimiters.is_empty() && projected.split("<<").skip(1).any(|tail| {
        let tail = tail.strip_prefix('-').unwrap_or(tail).trim_start_matches([' ', '\t']);
        // Stop at an unquoted word boundary, preserving quoted blanks and
        // fragments such as E$"O"F in the same raw delimiter word.
        let mut quote = None;
        let mut escaped = false;
        let end = tail.char_indices().find_map(|(index, ch)| {
            if escaped {
                escaped = false;
            } else if ch == '\\' && quote != Some('\'') {
                escaped = true;
            } else if quote == Some(ch) {
                quote = None;
            } else if quote.is_none() {
                if matches!(ch, '\'' | '"') {
                    quote = Some(ch);
                } else if ch.is_whitespace() || matches!(ch, ';' | '|' | '&' | '<' | '>' | '(' | ')') {
                    return Some(index);
                }
            }
            None
        }).unwrap_or(tail.len());
        tail[..end].contains("$'") || tail[..end].contains("$\"")
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
    let _numeric_locals = NumericLocalsScope::enter(numeric_locals(&projected));
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
    let (tokens, literal_words, source_ends) = (lexed.tokens, lexed.literals, lexed.ends);
    let captured_status: HashSet<String> = tokens
        .iter()
        .filter_map(|token| match token {
            ShellToken::Word(word) => {
                let (name, value) = word.split_once('=')?;
                (value == "$?" && is_shell_variable_name(name)).then(|| name.to_string())
            }
            _ => None,
        })
        .collect();
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
    let mut directory_stack = Vec::new();
    // Credential reads only. A function or coprocess body runs later or never,
    // and a piped or background compound command runs in a subshell. Their
    // extents are not modelled, so each directory change is only possible.
    let read_mode = effects.read_contexts.is_some();
    let deferred_bodies = read_mode
        && (tokens.windows(2).any(|pair| pair == [ShellToken::Operator('('), ShellToken::Operator(')')])
            || tokens.iter().any(|token| matches!(token,
                ShellToken::Word(word) if matches!(word.as_str(), "function" | "coproc"))));
    effects.deferred_bodies |= deferred_bodies;
    let unordered_cwd = deferred_bodies
        || (read_mode
            && tokens.iter().any(|token| matches!(token, ShellToken::Word(word)
                if matches!(word.as_str(), "{" | "if" | "while" | "until" | "for" | "select" | "case")))
            && (0..tokens.len()).any(|at| lone_operator(&tokens, &projected, &source_ends, at).is_some()));
    // Directories and stack on entry to the current and-or list, which a
    // trailing background `&` runs in a subshell.
    // `cd -` returns to the previous directory; unknown before the first change.
    let mut previous_dirs: Directories = vec![None];
    let mut list_entry = (dirs.clone(), directory_stack.clone(), previous_dirs.clone());
    // The directories on entry to each open `case`. Only one arm runs, so no arm
    // sees a `cd` from an earlier one and any of them may be the one that ran.
    let mut case_entries: Vec<Directories> = Vec::new();
    let mut continuation = false;
    // Credential reads: the reader and operands before an unquoted argument
    // substitution, which the words after it continue.
    let mut continuation_head = String::new();
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
            // The `&` of `>&file` and the `|` of `>|file` belong to the redirect.
            (CaseSyntax::Code, Some(ShellToken::Operator('&' | '|')))
                if end > 0 && matches!(tokens[end - 1], ShellToken::Operator('<' | '>')) =>
            {
                continue;
            }
            (CaseSyntax::Code, Some(ShellToken::Operator(ch @ (';' | '&' | '|' | '(' | ')')))) => Some(*ch),
            (CaseSyntax::Code, None) => None,
            (CaseSyntax::Code, _) => continue,
        };
        // Credential reads: only a conditional block still open before a
        // statement can skip it, including a `{ }` group after `&&` or `||`.
        // A closer counts only when spelled unquoted in command position.
        let before = &tokens[..start.min(tokens.len())];
        let mut open_blocks: Vec<bool> = Vec::new();
        for (at, token) in before.iter().enumerate() {
            if case_tokens[at] == CaseSyntax::Esac {
                open_blocks.pop();
                continue;
            }
            let ShellToken::Word(word) = token else { continue };
            let closer = case_tokens[at] == CaseSyntax::Code
                && at > 0
                && matches!(before[at - 1], ShellToken::Operator(';' | '&'))
                && projected.get(source_ends[at - 1]..source_ends[at]).is_some_and(|source| source.trim_start() == word);
            match word.as_str() {
                "if" | "while" | "until" | "for" | "select" | "case" => open_blocks.push(true),
                "{" => open_blocks.push(at > 0 && matches!(before[at - 1], ShellToken::Operator('&' | '|'))),
                "fi" | "done" | "}" if closer => {
                    open_blocks.pop();
                }
                _ => {}
            }
        }
        let conditional_before = open_blocks.contains(&true);
        let assignment_may_skip = if read_mode { conditional_before } else { conditional_bindings }
            || matches!(
                tokens.get(start.wrapping_sub(1)),
                Some(ShellToken::Operator('&' | '|'))
            );
        // Pattern text and `esac` are not operands of anything.
        let visible: Vec<usize> =
            (start..end).filter(|index| case_tokens[*index] == CaseSyntax::Code).collect();
        let visible_tokens: Vec<ShellToken> = visible.iter().map(|&index| tokens[index].clone()).collect();
        // Preserve original argv spelling (notably numeric redirect fds).
        // The context is paired here with its source span, never by argv match.
        let source_start = start.checked_sub(1).map_or(0, |index| source_ends[index]);
        let source_end = source_ends.get(end).map_or(projected.len(), |end| end - 1);
        let mut read_command = projected[source_start..source_end].to_string();
        let mut statement = visible_tokens.as_slice();
        let mut skipped_keywords = 0;
        // After a pipe or background `&` (past newlines), a directory change
        // may not reach the next command; before a pipe it never does.
        let keep_alternatives = unordered_cwd
            || (read_mode
                && (0..start).rev().find(|&at| tokens[at] != ShellToken::Operator(';')).is_some_and(
                    |at| match &tokens[at] {
                        ShellToken::Operator('|') => true,
                        ShellToken::Operator('&') => at == 0 || tokens[at - 1] != ShellToken::Operator('&'),
                        _ => false,
                    },
                ));
        let pipe_element = read_mode && lone_operator(&tokens, &projected, &source_ends, end) == Some('|');
        start = end + 1;
        while matches!(statement.first(), Some(ShellToken::Word(word))
            if matches!(word.as_str(), "{" | "if" | "then" | "else" | "elif" | "do" | "while" | "until" | "!"))
        {
            statement = &statement[1..];
            skipped_keywords += 1;
        }
        let substitution_scope = separator == Some('(')
            && matches!(statement.last(), Some(ShellToken::Word(word)) if word.ends_with('$'));
        // The substitution's output stands in as one opaque operand.
        let substitution_head = substitution_scope.then(|| format!(
            "{}{}SUBSTITUTION",
            if continuation { continuation_head.as_str() } else { "" },
            read_command.strip_suffix('$').unwrap_or(&read_command),
        ));
        let mut incoming = dirs.clone();
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
                read_command.push('\n');
                read_command.push_str(&body);
                read_command.push_str(&delimiter.value);
                read_command.push('\n');
                command.push('\n');
                command.push_str(&body);
                command.push_str(&delimiter.value);
                command.push('\n');
                if !delimiter.literal_body {
                    expanding_bodies.extend(heredoc_substitution_bodies(&body, &delimiter)?);
                }
            }
            let redirects = redirect_words(&statement);
            let words: Vec<String> = statement
                .iter()
                .enumerate()
                .filter_map(|(index, token)| match token {
                    ShellToken::Word(word) if !redirects[index] => Some(word.clone()),
                    _ => None,
                })
                .collect();
            let word_literals: Vec<bool> = statement
                .iter()
                .enumerate()
                .filter_map(|(index, token)| {
                    (matches!(token, ShellToken::Word(_)) && !redirects[index])
                        .then_some(statement_literals[index])
                })
                .collect();
            // A redirect's filename is no operand, but its substitutions still run.
            let redirect_targets: Vec<(&str, bool)> = statement
                .iter()
                .enumerate()
                .filter_map(|(index, token)| match token {
                    ShellToken::Word(word) if redirects[index] => {
                        Some((word.as_str(), statement_literals[index]))
                    }
                    _ => None,
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
                        numeric_test_operand(&args[at], &captured_status)?;
                        numeric_test_operand(
                            args.get(at + 2)
                                .ok_or("an unresolved conditional operand")?,
                            &captured_status,
                        )?;
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
            let every_word = words
                .iter()
                .map(String::as_str)
                .zip(word_literals.iter().copied())
                .chain(redirect_targets.iter().copied());
            for (word, literal) in every_word {
                if !literal {
                    for body in substitution_bodies(word)? {
                        collect_script(
                            &body,
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
                        if matches!(name, Some("mv" | "cp" | "install"))
                            && words.last().is_some_and(|destination| destination == &raw)
                        {
                            effects.directory_sinks.insert(path.clone());
                        }
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

            // A leading command executes before any later conditional syntax.
            let entry_directory = depth == 0 && skipped_keywords == 0
                && effects.read_contexts.as_ref().is_some_and(Vec::is_empty);
            if depth == 0 && let Some(contexts) = &mut effects.read_contexts {
                if dirs.is_empty() { return Err("an unresolved credential working directory"); }
                let command = if continuation { format!("{continuation_head}{read_command}") } else { read_command };
                contexts.push(ShellReadContext { command, directories: dirs.clone() });
            }
            let read_stack = effects.read_contexts.is_some();
            let read_unconditional =
                read_stack && !keep_alternatives && (!assignment_may_skip || entry_directory);
            let statement_entry = (dirs.clone(), directory_stack.clone());
            if read_stack && name == Some("popd") {
                if let Some(previous) = directory_stack.pop() {
                    if keep_alternatives {
                        for directory in previous {
                            add_directory(&mut dirs, directory);
                        }
                    } else {
                        dirs = previous;
                    }
                }
            }
            if name == Some("cd") || (read_stack && name == Some("pushd")) {
                if name == Some("pushd") { directory_stack.push(dirs.clone()); }
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
                        // Credential reads only: a glob component names one
                        // unknown entry, so the directory keeps its literal
                        // ancestors. A dot-glob may match `..` and stays unresolved.
                        let masked;
                        let target = match read_mode && !target.contains(['$', '`']) {
                            true => match target.split('/').map(|part| match part.contains(['*', '?', '[']) {
                                false => Some(part),
                                true => (!part.starts_with('.')).then_some(UNKNOWN_DIRECTORY),
                            }).collect::<Option<Vec<_>>>() {
                                Some(parts) => {
                                    masked = parts.join("/");
                                    masked.as_str()
                                }
                                None => target.as_str(),
                            },
                            false => target.as_str(),
                        };
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
                if read_stack && name == Some("cd") && operands.len() == 1 && operands[0] == "-" {
                    changed = previous_dirs.clone();
                }
                // The next && statement requires cd to have succeeded.
                // Keep failed/skipped alternatives for later ;/|| joins.
                if (and_then && !keep_alternatives) || read_unconditional {
                    dirs = changed;
                } else {
                    for directory in changed {
                        add_directory(&mut dirs, directory);
                    }
                }
            }

            // Keep the credential guard's existing destination policy for an
            // unconditional directory command, including a later ; join.
            // Conditional/skipped commands retain the shared alternatives.
            if read_unconditional && matches!(name, Some("cd" | "pushd")) {
                incoming = dirs.clone();
            }
            // A piped change never reaches this shell; one that may be
            // skipped or not persist only adds a possible previous directory.
            if read_stack && !pipe_element && matches!(name, Some("cd" | "pushd" | "popd"))
                && dirs != statement_entry.0
            {
                if !((and_then && !keep_alternatives) || read_unconditional) {
                    for directory in statement_entry.0.clone() {
                        add_directory(&mut previous_dirs, directory);
                    }
                } else {
                    previous_dirs = statement_entry.0.clone();
                }
            }
            if pipe_element {
                (dirs, directory_stack) = statement_entry;
            }
            if name == Some("cd") || (read_stack && matches!(name, Some("pushd" | "popd"))) {
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
                    directory_stack.clone(),
                    list_entry.clone(),
                    previous_dirs.clone(),
                    (std::mem::take(&mut continuation_head), substitution_head.unwrap_or_default()),
                ));
                continuation = false;
            }
            Some(')') => {
                let parent = scopes.pop().ok_or("an unmatched shell scope")?;
                dirs = parent.0;
                alternatives = parent.1;
                directory_values = parent.2;
                continuation = parent.3;
                directory_stack = parent.4;
                list_entry = parent.5;
                previous_dirs = parent.6;
                // A substitution's following words continue its command.
                let (outer, after) = parent.7;
                continuation_head = if continuation { after } else { outer };
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
                if read_mode && !unordered_cwd && lone_operator(&tokens, &projected, &source_ends, end).is_some() {
                    alternatives.clear();
                    (dirs, directory_stack, previous_dirs) = list_entry.clone();
                }
                for directory in std::mem::take(&mut alternatives) {
                    add_directory(&mut dirs, directory);
                }
            }
            _ => {}
        }
        if matches!(separator, None | Some(';' | '('))
            || lone_operator(&tokens, &projected, &source_ends, end) == Some('&')
        {
            list_entry = (dirs.clone(), directory_stack.clone(), previous_dirs.clone());
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
