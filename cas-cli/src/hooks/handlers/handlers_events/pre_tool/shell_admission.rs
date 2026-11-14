//! Shared source-preserving shell scanner and invocation grammar.
use super::shell_effects;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ShellToken {
    Word(String),
    Operator(char),
}

// Argument-only consumers omit attached IO numbers. Security/effect consumers
// retain the original tokens, including every redirect target.
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
fn shell_ansi_c_word(command: &str, start: usize) -> Option<(String, usize)> {
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
                    b'a' => 7, b'b' => 8, b'e' | b'E' => 27, b'f' => 12,
                    b'n' => b'\n', b'r' => b'\r', b't' => b'\t', b'v' => 11,
                    b'\\' | b'\'' | b'"' | b'?' => escape,
                    b'x' | b'0'..=b'7' => {
                        let (radix, limit, mut value, mut digits) = if escape == b'x' {
                            (16, 2, 0, 0)
                        } else {
                            (8, 3, u32::from(escape - b'0'), 1)
                        };
                        while digits < limit {
                            let Some(digit) = bytes.get(index).and_then(|ch| (*ch as char).to_digit(radix)) else { break };
                            value = value * radix + digit;
                            digits += 1;
                            index += 1;
                        }
                        if digits == 0 { return None; }
                        value as u8
                    }
                    _ => return None,
                };
                if value == 0 { return None; }
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
                     ends: &mut Vec<usize>, end: usize| {
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
                    &mut ends, index,
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
        if quote == Some('"') && expansions.is_empty() && ch == '\\'
            && command.as_bytes().get(index + 1..index + 3) == Some(b"${")
        {
            word.push_str("\\$");
            chars.next();
            continue;
        }
        if let Some(expansion) = expansions.last_mut() && expansion.escaped {
            word.push(ch);
            expansion.escaped = false;
            continue;
        }
        let active_quote = expansions.last().map_or(quote, |expansion| expansion.quote);
        // Single quotes inside a double-quoted parameter default differ
        // across Bash modes. Retain potentially active nested operands too.
        if quote == Some('"') && active_quote == Some('\'') && !expansions.is_empty()
            && ch == '$' && command.as_bytes().get(index + 1) == Some(&b'{')
            && command.as_bytes()[..index].iter().rev().take_while(|&&ch| ch == b'\\').count() % 2 == 0
        {
            let end = command[index + 2..].find('}').map_or(command.len(), |end| index + 2 + end);
            parameters.push((index + 2, end));
        }
        // Shell line continuations disappear before word matching, including
        // inside double quotes; single-quoted backslash/newline stays data.
        if active_quote != Some('\'') && ch == '\\'
            && chars.peek().is_some_and(|(_, next)| *next == '\n')
        {
            chars.next();
            continue;
        }
        if quote.is_none() && expansions.is_empty() && ch == '$'
            && chars.peek().is_some_and(|(_, next)| *next == '\'')
        {
            if let Some((decoded, end)) = shell_ansi_c_word(command, index) {
                word.push_str(&decoded);
                for _ in command[index + 1..end].chars() { chars.next(); }
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
                for _ in 0..consumed { chars.next(); }
                expansions.push(Expansion {
                    start: index, opening, closing, balance, quote: None, escaped: false,
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
                for _ in 1..substitution.chars().count() { chars.next(); }
                literal = false;
                continue;
            }
            unresolved_at.get_or_insert(index);
        }
        // A substitution has its own quoting context: its double quotes must
        // not close the outer word or turn quoted parentheses into scopes.
        // Preserve its source for the recursive effect collector, which still
        // judges both the commands it runs and how its output is used.
        if active_quote != Some('\'') && (quote == Some('"') || !expansions.is_empty())
            && ch == '$' && chars.peek().is_some_and(|(_, ch)| *ch == '(') {
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
                Some('\'') => {},
                _ if ch == '\\' => expansion.escaped = true,
                Some(_) => {},
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
                _ => {},
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
            Some('"') if ch == '\\'
                && chars.peek().is_some_and(|(_, next)| matches!(next, '`' | '\\')) =>
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
                        &mut ends, index,
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
                        &mut ends, index,
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
                    &mut ends, index,
                ),
                _ => word.push(ch),
            },
        }
    }
    if escaped {
        word.push('\\');
    }
    if let Some(expansion) = expansions.first() {
        unresolved_at = Some(unresolved_at.map_or(expansion.start, |at: usize| at.min(expansion.start)));
    }
    push_word(
        &mut tokens,
        &mut literals,
        &mut word,
        &mut literal,
        &mut word_quoted,
        &mut ends, command.len(),
    );
    ShellLexed { tokens, literals, ends, unresolved_at, parameters }
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
        if shell && matches!(arg.as_str(), "-o" | "+o" | "-O" | "+O" | "--rcfile" | "--init-file") {
            at += 1;
        }
        at += 1;
    }
    None
}
