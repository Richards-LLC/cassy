//! Shared source-preserving shell scanner and invocation grammar.
use super::shell_effects;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ShellToken {
    Word(String),
    Operator(char),
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
    let mut tokens = Vec::new();
    let mut literals = Vec::new();
    let mut ends = Vec::new();
    let mut unresolved_at = None;
    let mut parameters = Vec::new();
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
        if !word.is_empty() || (arguments_only && *word_quoted) {
            tokens.push(ShellToken::Word(std::mem::take(word)));
            literals.push(*literal);
            ends.push(end);
            *literal = true;
        }
        *word_quoted = false;
    };
    let mut chars = command.char_indices().peekable();
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
            && command.as_bytes().get(index + 1..index + 3) == Some(b"${")
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
        if active_quote != Some('\'') && ch == '`' {
            let bytes = command.as_bytes();
            let mut end = index + 1;
            while end < bytes.len() && bytes[end] != b'`' {
                end += if bytes[end] == b'\\' { 2 } else { 1 };
            }
            if end < bytes.len() {
                let substitution = &command[index..=end];
                word.push_str(substitution);
                for _ in 1..substitution.chars().count() {
                    chars.next();
                }
                literal = false;
                continue;
            }
            unresolved_at.get_or_insert(index);
        }
        // A substitution has its own quoting context: its double quotes must
        // not close the outer word or turn quoted parentheses into scopes.
        // Preserve its source for the recursive effect collector, which still
        // judges both the commands it runs and how its output is used.
        if active_quote != Some('\'')
            && (quote == Some('"') || !expansions.is_empty())
            && ch == '$'
            && chars.peek().is_some_and(|(_, ch)| *ch == '(')
        {
            let tail = &command[index..];
            if let Some(end) = shell_effects::literal_cat_heredoc_end(&tail)
                .or_else(|| shell_effects::substitution_end(tail.as_bytes(), 2, depth + 1))
            {
                let substitution = &tail[..=end];
                word.push_str(substitution);
                for _ in 1..substitution.chars().count() {
                    chars.next();
                }
                literal = false;
                continue;
            } else {
                unresolved_at.get_or_insert(index);
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
    }
    if let Some(expansion) = expansions.first() {
        unresolved_at =
            Some(unresolved_at.map_or(expansion.start, |at: usize| at.min(expansion.start)));
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
    }
}

/// Resolve shell input before inspecting arguments as program text. A literal
/// script consumes the remaining argv; its -c arguments belong to that script.
/// Wrapper programs retain their own option grammar (su may name a user first).
pub(super) fn shell_payload_index(name: &str, args: &[String]) -> Option<usize> {
    let shell = matches!(name, "sh" | "bash" | "dash" | "zsh" | "ksh");
    let mut at = 0;
    while let Some(arg) = args.get(at) {
        if arg == "--" || (shell && (!arg.starts_with(['-', '+']) || arg == "-")) {
            return None;
        }
        if arg == "--command"
            || (arg.starts_with('-') && !arg.starts_with("--") && arg.contains('c'))
        {
            return Some(at + 1);
        }
        // These options consume a value, which cannot itself become -c.
        if shell
            && matches!(
                arg.as_str(),
                "-o" | "+o" | "-O" | "+O" | "--rcfile" | "--init-file"
            )
        {
            at += 1;
        }
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
    pub(super) fn argument_tokens(&self, source: &str) -> Vec<ShellToken> {
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
                Some(token.clone())
            })
            .peekable();
        let mut arguments = Vec::new();
        while let Some(token) = tokens.next() {
            if token == ShellToken::Operator('&')
                && tokens.peek() == Some(&ShellToken::Operator('>'))
            {
                continue;
            }
            if matches!(token, ShellToken::Operator('>' | '<')) {
                while matches!(
                    tokens.peek(),
                    Some(ShellToken::Operator('>' | '<' | '&' | '|'))
                ) {
                    tokens.next();
                }
                if matches!(tokens.peek(), Some(ShellToken::Word(_))) {
                    tokens.next();
                }
                continue;
            }
            arguments.push(token);
        }
        arguments
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
