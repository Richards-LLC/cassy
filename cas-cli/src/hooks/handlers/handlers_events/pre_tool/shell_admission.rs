//! Shared source-preserving shell scanner and invocation grammar.
use super::{HeredocDelimiter, heredoc_delimiters, shell_command_projection, shell_heredoc_bodies};

pub(super) const MAX_DEPTH: usize = 6;

/// A refusal names a parser rule and original byte position, never a guessed
/// filesystem effect. Construct labels deliberately contain no argument values.
#[derive(Clone, Debug)]
pub(super) struct Unsupported {
    pub(super) rule: &'static str,
    pub(super) offset: usize,
    construct: &'static str,
    why: &'static str,
    alternative: &'static str,
}

impl Unsupported {
    fn execution(rule: &'static str, offset: usize, construct: &'static str, why: &'static str, alternative: &'static str) -> Self {
        Self { rule, offset, construct, why, alternative }
    }

    fn unterminated(offset: usize, construct: &'static str) -> Self {
        Self { rule: "shell.unterminated", offset, construct,
            why: "the construct is incomplete", alternative: "close the construct" }
    }

    pub(super) fn depth(offset: usize) -> Self {
        Self { rule: "shell.depth", offset, construct: "nested shell",
            why: "nesting exceeds six executing scopes",
            alternative: "flatten the command or use a literal script file" }
    }
}

impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SHELL CONSTRUCT REFUSED [{}] at byte {} ({}): {}. Instead: {}.",
            self.rule, self.offset, self.construct, self.why, self.alternative)
    }
}

/// All consumers use this escape-aware boundary; the body is decoded only
/// after its exact source span is known.
pub(super) fn backquote_end(bytes: &[u8], opening: usize) -> Option<usize> {
    let mut end = opening + 1;
    while let Some(&byte) = bytes.get(end) {
        if byte == b'`' { return Some(end); }
        end += if byte == b'\\' { 2 } else { 1 };
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ShellToken {
    Word(String),
    Operator(char),
}

/// Cooked argv keeps its source provenance when redirections and shell
/// grouping are projected away. Quoted data cannot manufacture command syntax.
pub(super) struct Statement {
    pub(super) words: Vec<String>,
    literals: Vec<bool>,
    offsets: Vec<usize>,
    assignments: Vec<bool>,
    /// Reserved syntax is recognized from raw source, never a cooked word.
    prefix: usize,
}

impl Statement {
    pub(super) fn policy_words(&self) -> &[String] {
        &self.words[self.prefix..]
    }

    pub(super) fn worker_refusal(&self, index: Option<usize>, xargs: bool) -> Option<Unsupported> {
        use super::{is_shell_variable_name, shell_word_basename};
        let words = &self.words;
        let offset = |at: usize| self.offsets.get(at).copied().unwrap_or(0);
        let dynamic = |at: usize| !self.literals.get(at).copied().unwrap_or(false);
        let deferred = |at| Unsupported::execution("shell.deferred_code", offset(at), "deferred shell code",
            "the action runs later as code", "run the command directly");
        let input = |at| Unsupported::execution("shell.stdin_interpreter", offset(at), "shell input stream",
            "the program text comes from input instead of a literal argument",
            "use a literal script file or a literal shell -c payload");
        let payload = |at| Unsupported::execution("shell.dynamic_eval", offset(at), "dynamic shell payload",
            "the program text is unknown until run time", "use a literal payload or script path");

        // Reserved words are recognized only at the command prefix. A wrapper
        // option value or an ordinary argument named coproc is inert data.
        for (at, word) in words.iter().enumerate() {
            if word == "coproc" {
                return Some(Unsupported::execution("shell.coproc", offset(at), "coprocess",
                    "the named and unnamed forms have ambiguous command boundaries",
                    "use a pipeline or a background job"));
            }
            if !matches!(word.as_str(), "!" | "{" | "if" | "then" | "else" | "elif" | "do" | "while" | "until")
                && !word.split_once('=').is_some_and(|(name, _)| is_shell_variable_name(name))
            { break; }
        }
        for at in self.prefix..words.len() {
            if !self.assignments[at] { break; }
            if assignment_name(&words[at]).is_some_and(|name| matches!(name, "PS4" | "PROMPT_COMMAND")) {
                return Some(deferred(at));
            }
        }
        let index = index?;
        if dynamic(index) {
            return Some(Unsupported::execution("shell.dynamic_command", offset(index), "dynamic command word",
                "the executable is unknown until run time", "name the program literally"));
        }
        let name = shell_word_basename(&words[index]);
        let args = &words[index + 1..];
        // xargs appends runtime input to argv. Only data-only consumers and
        // process-signal consumers with their own existing policy can accept
        // that unknown input. Filename readers/writers and unknown programs
        // need explicit operands so credential and write-root policy can act.
        if xargs && !matches!(name,
            "echo" | "printf" | "true" | "false" | "test" | "[" | "basename" | "dirname" | "tr"
            | "kill" | "pgrep" | "pkill" | "killall")
        {
            return Some(Unsupported::execution("shell.dynamic_command", offset(index), "input-derived command arguments",
                "runtime input supplies filenames or arguments whose effects cannot be resolved",
                "use explicit literal operands or a data-only consumer"));
        }
        if matches!(name, "export" | "declare" | "typeset" | "local" | "readonly") {
            for (at, word) in args.iter().enumerate() {
                if assignment_name(word).is_some_and(|name| matches!(name, "PS4" | "PROMPT_COMMAND")) {
                    return Some(deferred(index + 1 + at));
                }
            }
        }
        match name {
            "alias" => return Some(deferred(index)),
            "trap" if args.iter().find(|arg| arg.as_str() != "--")
                .is_some_and(|action| !action.is_empty() && action.as_str() != "-" && !action.starts_with('-')) => return Some(deferred(index)),
            "shopt" if args.iter().any(|arg| arg == "expand_aliases") && args.iter().any(|arg| arg.starts_with('-') && arg.contains('s')) => return Some(deferred(index)),
            "eval" if (index + 1..words.len()).any(dynamic) => return Some(payload(index)),
            "source" | "." => {
                let at = index + 1 + usize::from(args.first().is_some_and(|arg| arg == "--"));
                match words.get(at).map(String::as_str) {
                    None | Some("-" | "/dev/stdin" | "/dev/fd/0" | "/proc/self/fd/0") => return Some(input(index)),
                    Some(_) if dynamic(at) => return Some(payload(at)),
                    _ => {}
                }
            }
            "sh" | "bash" | "dash" | "zsh" | "ksh" => match shell_input(args) {
                ShellInput::Stdin => return Some(input(index)),
                ShellInput::Command(at) | ShellInput::Script(at) if dynamic(index + 1 + at) => return Some(payload(index + 1 + at)),
                _ => {}
            },
            _ => {}
        }
        None
    }
}

enum ShellInput { Command(usize), Script(usize), Stdin, Display }

/// The name bound by scalar, append or array-element assignment syntax. A
/// declaration builtin interprets its cooked argv; command prefixes additionally
/// require an unquoted raw lhs (recorded by the statement projection).
fn assignment_name(word: &str) -> Option<&str> {
    let (lhs, _) = word.split_once('=')?;
    let lhs = lhs.strip_suffix('+').unwrap_or(lhs);
    let name = if let Some((name, subscript)) = lhs.split_once('[') {
        if !subscript.ends_with(']') { return None; }
        name
    } else { lhs };
    super::is_shell_variable_name(name).then_some(name)
}

/// Classify shell input once. After a script operand, -c is script data;
/// after -s, operands are positional arguments rather than a script filename.
fn shell_input(args: &[String]) -> ShellInput {
    let mut at = 0;
    let mut stdin = false;
    while let Some(arg) = args.get(at) {
        if matches!(arg.as_str(), "--help" | "--version") { return ShellInput::Display; }
        if arg == "--" {
            return if stdin || args.get(at + 1).is_none() { ShellInput::Stdin } else { ShellInput::Script(at + 1) };
        }
        if !arg.starts_with(['-', '+']) || arg == "-" {
            return if stdin || arg == "-" { ShellInput::Stdin } else { ShellInput::Script(at) };
        }
        if arg == "--command" || (arg.starts_with('-') && !arg.starts_with("--") && arg.contains('c')) {
            return ShellInput::Command(at + 1);
        }
        if arg.starts_with('-') && !arg.starts_with("--") && arg.contains('s') { stdin = true; }
        if matches!(arg.as_str(), "-o" | "+o" | "-O" | "+O" | "--rcfile" | "--init-file") { at += 1; }
        at += 1;
    }
    ShellInput::Stdin
}

// Argument-only consumers omit attached IO numbers. Security/effect consumers
// retain the original tokens, including every redirect target.
#[derive(Clone)]
pub(super) struct ShellLexed {
    pub(super) tokens: Vec<ShellToken>,
    pub(super) literals: Vec<bool>,
    /// Byte after each token in the original source, including removed quotes.
    pub(super) ends: Vec<usize>,
    pub(super) unresolved_at: Option<usize>,
    /// Raw parameter bodies: numeric operand checks must retain source quoting.
    pub(super) parameters: Vec<(usize, usize)>,
    /// Executing substitutions, recorded before word cooking loses quotes.
    pub(super) substitutions: Vec<String>,
    pub(super) refusal: Option<Unsupported>,
}

/// ANSI-C quoting is a constant word part, not a parameter or command
/// substitution. Unsupported escapes, NULs and non-UTF8 bytes stay unresolved
/// so the effect guard refuses them rather than matching a different word.
pub(super) fn shell_ansi_c_word(command: &str, start: usize) -> Option<(String, usize)> {
    let bytes = command.as_bytes();
    if bytes.get(start..start + 2) != Some(b"$'") {
        return None;
    }
    let mut decoded = Vec::new();
    let mut index = start + 2;
    while let Some(&ch) = bytes.get(index) {
        index += 1;
        match ch {
            b'\'' => return Some((String::from_utf8(decoded).ok()?, index)),
            b'\\' => {
                let escape = *bytes.get(index)?;
                index += 1;
                let value = match escape {
                    b'a' => 7,
                    b'b' => 8,
                    b'e' | b'E' => 27,
                    b'f' => 12,
                    b'n' => b'\n',
                    b'r' => b'\r',
                    b't' => b'\t',
                    b'v' => 11,
                    b'\\' | b'\'' | b'"' | b'?' => escape,
                    b'x' | b'0'..=b'7' => {
                        let (radix, limit, mut value, mut digits) = if escape == b'x' {
                            (16, 2, 0, 0)
                        } else {
                            (8, 3, u32::from(escape - b'0'), 1)
                        };
                        while digits < limit {
                            let Some(digit) = bytes
                                .get(index)
                                .and_then(|ch| (*ch as char).to_digit(radix))
                            else {
                                break;
                            };
                            value = value * radix + digit;
                            digits += 1;
                            index += 1;
                        }
                        if digits == 0 {
                            return None;
                        }
                        value as u8
                    }
                    _ => return None,
                };
                if value == 0 {
                    return None;
                }
                decoded.push(value);
            }
            0 => return None,
            _ => decoded.push(ch),
        }
    }
    None
}

/// The same lexer backs ordinary commands and substitution boundaries. Source
/// positions avoid recovering syntax from cooked words or quoted/comment text.
pub(super) fn factory_shell_lex(command: &str, arguments_only: bool, depth: usize) -> ShellLexed {
    lex_source(command, command, arguments_only, depth)
}

/// The projection hides heredoc data from tokenization while source offsets
/// and substitution boundaries still refer to the complete original script.
fn lex_source(command: &str, projection: &str, arguments_only: bool, depth: usize) -> ShellLexed {
    let mut tokens = Vec::new();
    let mut literals = Vec::new();
    let mut ends = Vec::new();
    let mut unresolved_at = None;
    let mut parameters = Vec::new();
    let mut substitutions = Vec::new();
    let mut refusal = None;
    let mut substitution_until = 0;
    let mut quote_start = 0;
    let mut word = String::new();
    let mut literal = true;
    let mut word_quoted = false;
    let mut quote = None;
    let mut escaped = false;
    let mut comment = false;
    // Parameter and arithmetic expansions are word parts, not shell scopes.
    // Keep their source intact so the effect/substitution collectors can still
    // inspect executable substitutions nested inside them.
    struct Expansion {
        start: usize,
        opening: char,
        closing: char,
        balance: usize,
        quote: Option<char>,
        escaped: bool,
    }
    let mut expansions: Vec<Expansion> = Vec::new();
    let push_word = |tokens: &mut Vec<ShellToken>,
                     literals: &mut Vec<bool>,
                     word: &mut String,
                     literal: &mut bool,
                     word_quoted: &mut bool,
                     ends: &mut Vec<usize>,
                     end: usize| {
        // Argument consumers must preserve quoted empty values (e.g. --shuffle '').
        if !word.is_empty() || *word_quoted {
            let constant = *literal || matches!(word.as_str(), "[" | "[[");
            tokens.push(ShellToken::Word(std::mem::take(word)));
            literals.push(constant);
            ends.push(end);
            *literal = true;
        }
        *word_quoted = false;
    };
    let mut chars = projection.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        if comment {
            if matches!(ch, '\n' | '\r') {
                comment = false;
                push_word(
                    &mut tokens,
                    &mut literals,
                    &mut word,
                    &mut literal,
                    &mut word_quoted,
                    &mut ends,
                    index,
                );
                tokens.push(ShellToken::Operator(';'));
                literals.push(true);
                ends.push(index + ch.len_utf8());
            }
            continue;
        }
        if escaped {
            word.push(ch);
            escaped = false;
            continue;
        }
        // Escaped ${ is literal data inside a double-quoted word. Its braces
        // must not manufacture a parameter quoting context for later text.
        if quote == Some('"')
            && expansions.is_empty()
            && ch == '\\'
            && command.as_bytes().get(index + 1) == Some(&b'$')
        {
            word.push_str("\\$");
            chars.next();
            continue;
        }
        if let Some(expansion) = expansions.last_mut()
            && expansion.escaped
        {
            word.push(ch);
            expansion.escaped = false;
            continue;
        }
        let active_quote = expansions.last().map_or(quote, |expansion| expansion.quote);
        // Single quotes inside a double-quoted parameter default differ
        // across Bash modes. Retain potentially active nested operands too.
        if quote == Some('"')
            && active_quote == Some('\'')
            && !expansions.is_empty()
            && ch == '$'
            && command.as_bytes().get(index + 1) == Some(&b'{')
            && command.as_bytes()[..index]
                .iter()
                .rev()
                .take_while(|&&ch| ch == b'\\')
                .count()
                % 2
                == 0
        {
            let end = command[index + 2..]
                .find('}')
                .map_or(command.len(), |end| index + 2 + end);
            parameters.push((index + 2, end));
        }
        // Shell line continuations disappear before word matching, including
        // inside double quotes; single-quoted backslash/newline stays data.
        if active_quote != Some('\'')
            && ch == '\\'
            && chars.peek().is_some_and(|(_, next)| *next == '\n')
        {
            chars.next();
            continue;
        }
        if quote.is_none()
            && expansions.is_empty()
            && ch == '$'
            && chars.peek().is_some_and(|(_, next)| *next == '\'')
        {
            if let Some((decoded, end)) = shell_ansi_c_word(command, index) {
                word.push_str(&decoded);
                for _ in command[index + 1..end].chars() {
                    chars.next();
                }
                word_quoted = true;
                continue;
            }
            unresolved_at.get_or_insert(index);
        }
        if active_quote != Some('\'') && ch == '$' {
            let expansion = match command.as_bytes().get(index + 1..) {
                Some([b'{', ..]) => Some(('{', '}', 1, 1)),
                Some([b'[', ..]) => Some(('[', ']', 1, 1)),
                Some([b'(', b'(', ..]) => Some(('(', ')', 2, 2)),
                _ => None,
            };
            if let Some((opening, closing, balance, consumed)) = expansion {
                word.push_str(&command[index..index + consumed + 1]);
                for _ in 0..consumed {
                    chars.next();
                }
                expansions.push(Expansion {
                    start: index,
                    opening,
                    closing,
                    balance,
                    quote: None,
                    escaped: false,
                });
                literal = false;
                continue;
            }
        }
        let expands = active_quote != Some('\'')
            || (quote == Some('"') && !expansions.is_empty());
        if expands && ch == '`' {
            if let Some(end) = backquote_end(command.as_bytes(), index) {
                if index >= substitution_until {
                    substitutions.push(backquote_body(&command[index + 1..end]));
                    substitution_until = end + 1;
                }
                let substitution = &command[index..=end];
                word.push_str(substitution);
                while chars.peek().is_some_and(|(at, _)| *at <= end) { chars.next(); }
                literal = false;
                continue;
            }
            unresolved_at.get_or_insert(index);
            refusal.get_or_insert_with(|| Unsupported::unterminated(index, "backquote"));
        }
        if expands && ch == '$'
            && command.as_bytes().get(index + 1) == Some(&b'(')
            && command.as_bytes().get(index + 2) != Some(&b'(')
        {
            let boundary = literal_cat_heredoc_end(&command[index..])
                .map(|end| Ok(index + end))
                .unwrap_or_else(|| substitution_boundary(command.as_bytes(), index + 2, depth + 1));
            match boundary {
                Ok(end) => {
                    if index >= substitution_until {
                        substitutions.push(command[index + 2..end].to_owned());
                        substitution_until = end + 1;
                    }
                    // Keep legacy token projections until each consumer moves:
                    // unquoted substitutions retain their ordinary scope tokens.
                    if quote == Some('"') || !expansions.is_empty() {
                        word.push_str(&command[index..=end]);
                        while chars.peek().is_some_and(|(at, _)| *at <= end) { chars.next(); }
                        literal = false;
                        continue;
                    }
                }
                Err(error) => {
                    unresolved_at.get_or_insert(index);
                    refusal.get_or_insert(error);
                }
            }
        }
        if let Some(expansion) = expansions.last_mut() {
            word.push(ch);
            match expansion.quote {
                Some(active) if active == ch => expansion.quote = None,
                Some('\'') => {}
                _ if ch == '\\' => expansion.escaped = true,
                Some(_) => {}
                None if matches!(ch, '\'' | '"') => expansion.quote = Some(ch),
                None if ch == expansion.opening => expansion.balance += 1,
                None if ch == expansion.closing => {
                    expansion.balance -= 1;
                    if expansion.balance == 0 {
                        if expansion.opening == '{' {
                            parameters.push((expansion.start + 2, index));
                        }
                        expansions.pop();
                    }
                }
                _ => {}
            }
            continue;
        }
        if quote != Some('\'') {
            if ch == '$'
                && chars
                    .peek()
                    .is_some_and(|(_, next)| matches!(next, '?' | '$' | '!'))
            {
                word.push(ch);
                word.push(chars.next().unwrap().1);
                continue;
            }
            if matches!(ch, '$' | '`') || (quote.is_none() && matches!(ch, '*' | '?' | '[')) {
                literal = false;
            }
        }
        match quote {
            Some(active) if ch == active => quote = None,
            Some('"') if ch == '\\' && chars.peek().is_some_and(|(_, next)| *next == '"') => {
                // An escaped double quote is word data, not the end of its quote context.
                word.push(chars.next().unwrap().1);
            }
            Some('"')
                if ch == '\\'
                    && chars
                        .peek()
                        .is_some_and(|(_, next)| matches!(next, '`' | '\\')) =>
            {
                // Escaped backticks are data; a doubled backslash leaves the
                // next backtick executable. Retain the source escape spelling
                // for consumers that inspect substitutions in dynamic words.
                word.push(ch);
                word.push(chars.next().unwrap().1);
            }
            Some(_) => word.push(ch),
            None => match ch {
                '\\' => {
                    escaped = true;
                    word_quoted = true;
                }
                '\'' | '"' => {
                    quote = Some(ch);
                    quote_start = index;
                    word_quoted = true;
                }
                '#' if word.is_empty() => comment = true,
                '>' | '<' | '|' | '&' | ';' | '(' | ')' => {
                    // Only an unquoted number attached to the operator is an
                    // IO descriptor: `2>log`, not the positional `2 >log`.
                    if arguments_only
                        && matches!(ch, '>' | '<')
                        && !word_quoted
                        && !word.is_empty()
                        && word.bytes().all(|c| c.is_ascii_digit())
                    {
                        word.clear();
                    }
                    push_word(
                        &mut tokens,
                        &mut literals,
                        &mut word,
                        &mut literal,
                        &mut word_quoted,
                        &mut ends,
                        index,
                    );
                    tokens.push(ShellToken::Operator(ch));
                    literals.push(true);
                    ends.push(index + ch.len_utf8());
                }
                '\n' | '\r' => {
                    push_word(
                        &mut tokens,
                        &mut literals,
                        &mut word,
                        &mut literal,
                        &mut word_quoted,
                        &mut ends,
                        index,
                    );
                    tokens.push(ShellToken::Operator(';'));
                    literals.push(true);
                    ends.push(index + ch.len_utf8());
                }
                ch if ch.is_whitespace() => push_word(
                    &mut tokens,
                    &mut literals,
                    &mut word,
                    &mut literal,
                    &mut word_quoted,
                    &mut ends,
                    index,
                ),
                _ => word.push(ch),
            },
        }
    }
    if escaped {
        word.push('\\');
        let at = command.len().saturating_sub(1);
        unresolved_at.get_or_insert(at);
        refusal.get_or_insert_with(|| Unsupported::unterminated(at, "escape"));
    }
    if quote.is_some() {
        unresolved_at.get_or_insert(quote_start);
        refusal.get_or_insert_with(|| Unsupported::unterminated(quote_start, "quote"));
    }
    if let Some(expansion) = expansions.first() {
        unresolved_at =
            Some(unresolved_at.map_or(expansion.start, |at: usize| at.min(expansion.start)));
    }
    if let Some(at) = unresolved_at {
        refusal.get_or_insert_with(|| Unsupported::unterminated(at, "expansion"));
    }
    push_word(
        &mut tokens,
        &mut literals,
        &mut word,
        &mut literal,
        &mut word_quoted,
        &mut ends,
        command.len(),
    );
    ShellLexed {
        tokens,
        literals,
        ends,
        unresolved_at,
        parameters,
        substitutions,
        refusal,
    }
}

/// Resolve shell input before inspecting arguments as program text. A literal
/// script consumes the remaining argv; its -c arguments belong to that script.
/// Wrapper programs retain their own option grammar (su may name a user first).
pub(super) fn shell_payload_index(name: &str, args: &[String]) -> Option<usize> {
    if matches!(name, "sh" | "bash" | "dash" | "zsh" | "ksh") {
        return match shell_input(args) { ShellInput::Command(at) => Some(at), _ => None };
    }
    let mut at = 0;
    while let Some(arg) = args.get(at) {
        if arg == "--" {
            return None;
        }
        if arg == "--command"
            || (arg.starts_with('-') && !arg.starts_with("--") && arg.contains('c'))
        {
            return Some(at + 1);
        }
        // These options consume a value, which cannot itself become -c.
        at += 1;
    }
    None
}

impl ShellLexed {
    pub(super) fn arithmetic_spans(
        &self,
    ) -> Result<Vec<(std::ops::Range<usize>, std::ops::Range<usize>)>, &'static str> {
        let lexed = self;
        let mut spans = Vec::new();
        let mut index = 0;
        while index + 1 < lexed.tokens.len() {
            if lexed.tokens[index..index + 2]
                != [ShellToken::Operator('('), ShellToken::Operator('(')]
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
            spans.push((index..end + 1, start..offset - 2));
            index = end + 1;
        }
        Ok(spans)
    }

    pub(super) fn erase_arithmetic(&mut self, tokens: std::ops::Range<usize>) {
        let offset = self.ends[tokens.end - 1];
        self.tokens
            .splice(tokens.clone(), [ShellToken::Word("true".into())]);
        self.literals.splice(tokens.clone(), [true]);
        self.ends.splice(tokens, [offset]);
    }

    /// Arguments projected from the same tokens and source spans used for
    /// effects. Only raw attached descriptors are IO numbers; quoted or
    /// separated numbers remain command arguments.
    fn argument_indices(&self, source: &str) -> Vec<usize> {
        let mut tokens = self
            .tokens
            .iter()
            .enumerate()
            .filter_map(|(at, token)| {
                if let ShellToken::Word(word) = token
                    && super::is_redirect_descriptor(word, self.tokens.get(at + 1))
                    && self.ends.get(at + 1) == Some(&(self.ends[at] + 1))
                {
                    let start = if at == 0 { 0 } else { self.ends[at - 1] };
                    if source[start..self.ends[at]].trim() == word {
                        return None;
                    }
                }
                Some((at, token))
            })
            .peekable();
        let mut arguments = Vec::new();
        while let Some((index, token)) = tokens.next() {
            if *token == ShellToken::Operator('&')
                && tokens.peek().is_some_and(|(_, token)| **token == ShellToken::Operator('>'))
            {
                continue;
            }
            if matches!(token, ShellToken::Operator('>' | '<')) {
                while matches!(
                    tokens.peek(),
                    Some((_, ShellToken::Operator('>' | '<' | '&' | '|')))
                ) {
                    tokens.next();
                }
                if matches!(tokens.peek(), Some((_, ShellToken::Word(_)))) {
                    tokens.next();
                }
                continue;
            }
            arguments.push(index);
        }
        arguments
    }

    pub(super) fn argument_tokens(&self, source: &str) -> Vec<ShellToken> {
        self.argument_indices(source).into_iter().map(|at| self.tokens[at].clone()).collect()
    }

    pub(super) fn statements(&self, source: &str, syntax: &[CaseSyntax]) -> Vec<Statement> {
        let indices: Vec<_> = self.argument_indices(source).into_iter()
            .filter(|&at| !matches!(syntax[at], CaseSyntax::Hidden | CaseSyntax::Esac)).collect();
        let tokens: Vec<_> = indices.iter().map(|&at| match syntax[at] {
            CaseSyntax::FirstArm | CaseSyntax::NextArm => ShellToken::Operator(';'),
            _ => self.tokens[at].clone(),
        }).collect();
        statement_indices(&tokens).into_iter().map(|statement| {
            let originals: Vec<_> = statement.into_iter().map(|at| indices[at]).collect();
            let mut prefix = 0;
            while let Some(&at) = originals.get(prefix) {
                let ShellToken::Word(word) = &self.tokens[at] else { break; };
                let start = if at == 0 { 0 } else { self.ends[at - 1] };
                if source[start..self.ends[at]].trim() != word { break; }
                if word == "function" && originals.get(prefix + 1).is_some() {
                    prefix += 2;
                } else if matches!(word.as_str(), "!" | "{" | "if" | "then" | "else" | "elif" | "do" | "while" | "until") {
                    prefix += 1;
                } else { break; }
            }
            Statement {
                prefix,
                assignments: originals.iter().map(|&at| {
                    let ShellToken::Word(word) = &self.tokens[at] else { return false; };
                    let start = if at == 0 { 0 } else { self.ends[at - 1] };
                    assignment_name(word).is_some() && word.split_once('=').is_some_and(|(lhs, _)| {
                        source[start..self.ends[at]].trim_start().starts_with(&format!("{lhs}="))
                    })
                }).collect(),
                words: originals.iter().map(|&at| match &self.tokens[at] {
                    ShellToken::Word(word) => word.clone(), _ => unreachable!(),
                }).collect(),
                literals: originals.iter().map(|&at| self.literals[at]).collect(),
                offsets: originals.iter().map(|&at| {
                    let start = if at == 0 { 0 } else { self.ends[at - 1] };
                    start + source[start..self.ends[at]].len() - source[start..self.ends[at]].trim_start().len()
                }).collect(),
            }
        }).collect()
    }

}

/// A shell arithmetic name is safe only after an unconditional local numeric
/// initializer, and while every possible writer preserves that fact. Compute
/// a conservative intersection across all later branches and loop iterations.
/// No ambient values, array indices or variable attributes are evaluated.
pub(super) fn numeric_bindings(
    lexed: &ShellLexed,
    source: &str,
) -> std::collections::HashSet<String> {
    use super::{
        is_shell_variable_name, shell_statements_from_tokens, shell_word_basename,
        unwrapped_command_index,
    };
    use std::collections::HashSet;
    let mut numeric_projection = lexed.clone();
    let Ok(spans) = lexed.arithmetic_spans() else {
        return HashSet::new();
    };
    for (tokens, _) in spans.into_iter().rev() {
        numeric_projection.erase_arithmetic(tokens);
    }
    let statements = shell_statements_from_tokens(numeric_projection.argument_tokens(source));
    let mut proven = HashSet::new();
    // A prefix of standalone assignments executes before every subsequent
    // arithmetic site. Later conditional initializers cannot establish proof.
    let mut start = 0;
    for end in 0..=lexed.tokens.len() {
        match lexed.tokens.get(end) {
            None | Some(ShellToken::Operator(';')) => {}
            Some(ShellToken::Operator(_)) => break,
            _ => continue,
        }
        let words = &lexed.tokens[start..end];
        if !lexed.literals[start..end].iter().all(|literal| *literal)
            || !words.iter().all(|token| {
                matches!(token, ShellToken::Word(word)
            if word.split_once('=').is_some_and(|(name, _)| is_shell_variable_name(name)))
            })
        {
            break;
        }
        for token in words {
            let ShellToken::Word(word) = token else {
                unreachable!()
            };
            let (name, value) = word.split_once('=').unwrap();
            if integer_expression(value, &HashSet::new())
                && value.bytes().any(|ch| ch.is_ascii_digit())
            {
                proven.insert(name.to_owned());
            } else {
                proven.remove(name);
            }
        }
        start = end + 1;
    }
    loop {
        let before = proven.len();
        for words in &statements {
            let index = unwrapped_command_index(words);
            let command = index.map(|at| shell_word_basename(&words[at]));
            let args = index.map(|at| &words[at + 1..]).unwrap_or(&[]);
            if command.is_some_and(|name| name.contains(['$', '`']))
                || matches!(
                    command,
                    Some(
                        "eval"
                            | "source"
                            | "."
                            | "declare"
                            | "typeset"
                            | "local"
                            | "integer"
                            | "getopts"
                            | "readarray"
                            | "mapfile"
                            | "alias"
                            | "trap"
                            | "shopt"
                            | "function"
                            | "fc"
                            | "history"
                            | "enable"
                            | "bind"
                            | "complete"
                            | "compgen"
                    )
                )
            {
                // These can introduce aliases, attributes, namerefs or an
                // unknown writer. A numeric value alone proves none of those.
                proven.clear();
                break;
            }
            let assignments = if matches!(command, Some("export" | "readonly")) {
                args
            } else {
                &words[..index.unwrap_or(words.len())]
            };
            for word in words
                .iter()
                .take(index.unwrap_or(words.len()) + usize::from(index.is_some()))
            {
                proven.retain(|name| {
                    !word.starts_with(&format!("{name}["))
                        && !word.starts_with(&format!("{name}+="))
                });
            }
            if command == Some("readonly")
                && args
                    .iter()
                    .any(|arg| arg.starts_with('-') && arg.contains(['a', 'A', 'n', 'i']))
            {
                proven.clear();
            }
            for word in assignments {
                if let Some((name, value)) = word.split_once('=') {
                    let name = name.trim_end_matches('+').split('[').next().unwrap_or(name);
                    if proven.contains(name) {
                        let arithmetic = value
                            .strip_prefix("$((")
                            .and_then(|value| value.strip_suffix("))"));
                        if word.starts_with(&format!("{name}+="))
                            || word.starts_with(&format!("{name}["))
                            || !integer_expression(arithmetic.unwrap_or(value), &proven)
                        {
                            proven.remove(name);
                        }
                    }
                }
            }
            fn invalidate(proven: &mut HashSet<String>, name: &str) {
                if super::is_shell_variable_name(name) {
                    proven.remove(name);
                } else {
                    proven.clear();
                }
            }
            match command {
                Some("for" | "select") => {
                    if let Some(name) = args.first() {
                        invalidate(&mut proven, name);
                    }
                }
                Some("unset") => {
                    for name in args.iter().filter(|arg| !arg.starts_with('-')) {
                        invalidate(&mut proven, name);
                    }
                }
                Some("read") => {
                    let mut at = 0;
                    let mut target = false;
                    while at < args.len() {
                        let arg = &args[at];
                        if arg == "--" {
                            at += 1;
                            break;
                        }
                        if !arg.starts_with('-') {
                            break;
                        }
                        if arg == "-a" {
                            at += 1;
                            if let Some(name) = args.get(at) {
                                invalidate(&mut proven, name);
                                target = true;
                            }
                        } else if matches!(
                            arg.as_str(),
                            "-d" | "-n" | "-N" | "-p" | "-t" | "-u" | "-i"
                        ) {
                            at += 1;
                        } else if arg.len() > 2
                            && arg[1..].contains(['a', 'd', 'n', 'N', 'p', 't', 'u', 'i'])
                        {
                            // Attached/combined value-taking read options are
                            // not a proven destination grammar.
                            proven.clear();
                            break;
                        }
                        at += 1;
                    }
                    for name in &args[at..] {
                        invalidate(&mut proven, name);
                        target = true;
                    }
                    if !target {
                        invalidate(&mut proven, "REPLY");
                    }
                }
                Some("printf") => {
                    if args.first().is_some_and(|arg| arg == "-v") {
                        if let Some(name) = args.get(1) {
                            invalidate(&mut proven, name);
                        }
                    } else if let Some(name) = args.first().and_then(|arg| arg.strip_prefix("-v")) {
                        invalidate(&mut proven, name);
                    }
                }
                _ => {}
            }
        }
        for &(start, end) in &lexed.parameters {
            let body = &source[start..end];
            let name_end = body
                .bytes()
                .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
                .count();
            if body[name_end..].starts_with('=') || body[name_end..].starts_with(":=") {
                proven.remove(&body[..name_end]);
            }
        }
        if proven.len() == before {
            return proven;
        }
    }
}

/// Recognize integer source, never recursively expand shell values. Names
/// require the proof above; brackets, indirection and substitutions stay out.
pub(super) fn integer_expression(
    expression: &str,
    proven: &std::collections::HashSet<String>,
) -> bool {
    let bytes = expression.as_bytes();
    let mut at = 0;
    while let Some(&ch) = bytes.get(at) {
        if ch.is_ascii_digit() || ch.is_ascii_whitespace() || b"()+-*/%<>=!&|^~?:,".contains(&ch) {
            at += 1;
        } else if ch == b'$' {
            at += 1;
            if bytes.get(at).is_some_and(|ch| b"?$!".contains(ch)) {
                at += 1;
                continue;
            }
            let braced = bytes.get(at) == Some(&b'{');
            if braced {
                at += 1;
            }
            let start = at;
            while bytes
                .get(at)
                .is_some_and(|ch| ch.is_ascii_alphanumeric() || *ch == b'_')
            {
                at += 1;
            }
            if start == at || !proven.contains(&expression[start..at]) {
                return false;
            }
            if braced {
                if bytes.get(at) != Some(&b'}') {
                    return false;
                }
                at += 1;
            }
        } else if ch.is_ascii_alphabetic() || ch == b'_' {
            let start = at;
            while bytes
                .get(at)
                .is_some_and(|ch| ch.is_ascii_alphanumeric() || *ch == b'_')
            {
                at += 1;
            }
            if !proven.contains(&expression[start..at]) {
                return false;
            }
        } else {
            return false;
        }
    }
    true
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


/// Resolve a substitution with the ordinary shell lexer. Only real operator
/// tokens can close it: quoted/comment text and case-pattern ')' are data.
pub(super) fn substitution_end(bytes: &[u8], index: usize, depth: usize) -> Option<usize> {
    substitution_boundary(bytes, index, depth).ok()
}

fn substitution_boundary(bytes: &[u8], index: usize, depth: usize) -> Result<usize, Unsupported> {
    if depth > MAX_DEPTH { return Err(Unsupported::depth(index.saturating_sub(2))); }
    let incomplete = || Unsupported::unterminated(index.saturating_sub(2), "substitution");
    let source = std::str::from_utf8(bytes.get(index..).ok_or_else(incomplete)?)
        .map_err(|_| incomplete())?;
    let projected = shell_command_projection(source, true);
    let lexed = lex_source(source, &projected, false, depth);
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
        let raw = source.get(start..end).ok_or_else(incomplete)?.trim();
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
                if let Some(mut error) = lexed.refusal.clone().filter(|error| error.offset < end) {
                    error.offset += index;
                    return Err(error);
                }
                return if cases.is_empty() { Ok(index + end - 1) } else { Err(incomplete()) };
            }
            ShellToken::Operator('<' | '>') => command_start = false,
            ShellToken::Operator(';' | '|' | '&') => command_start = !after_redirect(previous, end),
            _ => {}
        }
    }
    if let Some(mut error) = lexed.refusal {
        error.offset += index;
        Err(error)
    } else { Err(incomplete()) }
}



#[derive(Default)]
pub(super) struct ShellSubstitutions {
    pub(super) bodies: Vec<String>,
    pub(super) refusal: Option<Unsupported>,
}

impl ShellSubstitutions {
    pub(super) fn unresolved(&self) -> bool { self.refusal.is_some() }
}

pub(super) struct ParsedShell {
    pub(super) projected: String,
    pub(super) lexed: ShellLexed,
    pub(super) bodies: Vec<String>,
    pub(super) syntax: Vec<CaseSyntax>,
}

pub(super) fn parse(command: &str, depth: usize) -> Result<ParsedShell, Unsupported> {
    if depth > MAX_DEPTH { return Err(Unsupported::depth(0)); }
    let projected = shell_command_projection(command, true);
    let lexed = lex_source(command, &projected, false, depth);
    if let Some(error) = lexed.refusal.clone() { return Err(error); }
    let syntax = case_syntax(&projected, &lexed).map_err(|_| Unsupported::execution(
        "shell.unknown_syntax", 0, "case syntax", "the case boundaries cannot be resolved",
        "use complete case arms with literal patterns"))?;
    let mut found = ShellSubstitutions::default();
    let mut bodies = shell_heredoc_bodies(command).into_iter();
    for delimiter in projected.lines().flat_map(heredoc_delimiters) {
        let body = bodies.next().ok_or_else(|| Unsupported::unterminated(0, "heredoc"))?;
        if !delimiter.literal_body { collect_heredoc_substitutions(&body, &mut found); }
    }
    if let Some(error) = found.refusal { return Err(error); }
    found.bodies.extend(lexed.substitutions.iter().cloned());
    Ok(ParsedShell { projected, lexed, bodies: found.bodies, syntax })
}

pub(super) fn shell_substitutions(command: &str) -> ShellSubstitutions {
    match parse(command, 0) {
        Ok(parsed) => ShellSubstitutions { bodies: parsed.bodies, refusal: None },
        Err(error) => ShellSubstitutions { bodies: Vec::new(), refusal: Some(error) },
    }
}

/// Expanding heredoc text is data except for substitutions. Quotes in its
/// body do not suppress expansion; escaped markers and quoted delimiters do.
fn collect_heredoc_substitutions(body: &str, found: &mut ShellSubstitutions) {
    // The shell removes unescaped backslash-newline pairs before expanding
    // the body. Preserve escaped backslashes so marker escaping stays exact.
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
    let bytes = logical.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' {
            index += 2;
            continue;
        }
        let (start, end) = if bytes.get(index..index + 2) == Some(b"$(") {
            let end = literal_cat_heredoc_end(&logical[index..])
                .map(|end| index + end)
                .or_else(|| substitution_end(bytes, index + 2, 0));
            (index + 2, end)
        } else if bytes[index] == b'`' {
            let start = index + 1;
            (start, backquote_end(bytes, index))
        } else {
            index += 1;
            continue;
        };
        let Some(end) = end else {
            found.refusal = Some(Unsupported::unterminated(index, "heredoc expansion"));
            break;
        };
        found.bodies.push(if bytes[index] == b'`' {
            backquote_body(&logical[start..end])
        } else {
            logical[start..end].to_string()
        });
        index = end + 1;
    }
}


/// Whether this operator continues a redirect, as in `>&` and `>|`. The next word
/// is that redirect's filename, so it is not in command position and cannot be a
/// keyword. Bash runs `>case touch in /outside/p` as `touch in /outside/p`.
pub(super) fn after_redirect(previous: Option<(char, usize)>, end: usize) -> bool {
    matches!(previous, Some(('<' | '>', at)) if at + 1 == end)
}

/// Statement boundaries shared by policy and provenance consumers.
pub(super) fn statement_indices(tokens: &[ShellToken]) -> Vec<Vec<usize>> {
    let mut statements = Vec::new();
    let mut words: Vec<usize> = Vec::new();
    // One entry per open `(`: the enclosing statement an argument
    // substitution returns to, or `None` when `)` starts a new statement.
    let mut open: Vec<Option<Vec<usize>>> = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        match token {
            ShellToken::Word(_) => words.push(index),
            ShellToken::Operator('(') => {
                let argument_substitution = words.last().is_some_and(|&at| matches!(&tokens[at], ShellToken::Word(word) if word.ends_with('$')))
                    && super::unwrapped_command_index(&words.iter().map(|&at| match &tokens[at] { ShellToken::Word(word) => word.clone(), _ => unreachable!() }).collect::<Vec<_>>()).is_some_and(|index| index + 1 < words.len());
                if argument_substitution {
                    open.push(Some(std::mem::take(&mut words)));
                } else {
                    if !words.is_empty() {
                        statements.push(std::mem::take(&mut words));
                    }
                    open.push(None);
                }
            }
            ShellToken::Operator(')') => {
                if !words.is_empty() {
                    statements.push(std::mem::take(&mut words));
                }
                if let Some(Some(enclosing)) = open.pop() {
                    words = enclosing;
                }
            }
            ShellToken::Operator(';') | ShellToken::Operator('|') | ShellToken::Operator('&') => {
                if !words.is_empty() {
                    statements.push(std::mem::take(&mut words));
                }
            }
            ShellToken::Operator(_) => {}
        }
    }
    if !words.is_empty() {
        statements.push(words);
    }
    // An unclosed argument substitution still reports its enclosing words.
    for enclosing in open.into_iter().flatten().rev() {
        if !enclosing.is_empty() {
            statements.push(enclosing);
        }
    }
    statements
}

/// What a token means to the enclosing `case`. Code keeps its ordinary handling.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CaseSyntax {
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
pub(super) fn case_syntax(source: &str, lexed: &ShellLexed) -> Result<Vec<CaseSyntax>, &'static str> {
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
