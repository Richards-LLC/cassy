use crate::hooks::handlers::*;

// MessageDisplay Hook Handler
// ============================================================================
//
// CC 2.1.152+ fires MessageDisplay before assistant text reaches the terminal
// renderer. Cassy uses this to:
//
//   1. Detect and defuse React-Ink Box-in-Text crash triggers (nested fenced
//      code blocks) — see CLAUDE.md "Output hygiene" / cas-97ba.
//   2. Redact secret-shaped tokens from assistant text (API keys, bearer
//      tokens, etc.) when the guard is enabled.
//
// RISK: this hook rewrites assistant text. It ships DEFAULT OFF behind the
// config flag `[hooks] message_display_guard = true`. When the flag is absent
// or false the handler is a pure passthrough — byte-identical, zero-alloc.
//
// Invariants
// ----------
// - Never drops legitimate content: if the transform doesn't recognise a
//   shape, it leaves the text alone.
// - Only the offending span is modified, not surrounding prose.
// - Plain text with no trigger shapes returns `HookOutput::empty()` even
//   when the guard is on (no spurious `updatedMessage` in the output).

/// Handle the MessageDisplay hook event.
///
/// Returns `HookOutput::empty()` (passthrough) when:
///   - `[hooks] message_display_guard` is false/absent (default), OR
///   - the guard is on but the message contains no trigger shapes / secrets.
///
/// Returns `HookOutput::with_message_display_transform(sanitized)` when:
///   - the guard is on AND a nested-fence or secret pattern was found.
pub fn handle_message_display(
    input: &HookInput,
    cas_root: Option<&Path>,
) -> Result<HookOutput, MemError> {
    // Fast-path: no message text to inspect.
    //
    // cas-f3e3: this reads `display_text()`, not `input.message` directly.
    // Claude Code sends the assistant text under the wire key `delta` (now
    // aliased onto `message`), so the previous direct read was `None` on every
    // real MessageDisplay event and this handler returned here, unconditionally
    // — the whole guard below was unreachable in production. `display_text`
    // also declines non-final chunks, since a fence or a secret split across a
    // streaming boundary cannot be judged from one chunk.
    let message = match input.display_text() {
        Some(m) => m,
        None => return Ok(HookOutput::empty()),
    };

    // Gate: load config and check the opt-in flag.
    let guard_enabled = cas_root
        .and_then(|root| Config::load(root).ok())
        .map(|cfg| cfg.hooks().message_display_guard)
        .unwrap_or(false);

    if !guard_enabled {
        // Default-off: pure passthrough, zero allocations.
        return Ok(HookOutput::empty());
    }

    // Guard is ON — inspect for trigger shapes and redact secrets.
    let mut text = message.to_string();
    let mut modified = false;

    // 1. Sanitize nested fenced code blocks (Ink Box-in-Text crash trigger).
    if has_nested_fence(&text) {
        text = sanitize_nested_fences(text);
        modified = true;
    }

    // 2. Redact secret-shaped tokens.
    let (redacted, secrets_found) = redact_secrets(text.clone());
    if secrets_found {
        text = redacted;
        modified = true;
    }

    if modified {
        Ok(HookOutput::with_message_display_transform(text))
    } else {
        // Guard on, but nothing to transform — passthrough.
        Ok(HookOutput::empty())
    }
}

// ============================================================================
// Nested-fence detection & sanitization
// ============================================================================

/// Returns true when `text` contains a fenced code block (opened with ```)
/// that itself contains another labeled fenced block (``` + language tag),
/// which is the primary Ink Box-in-Text crash trigger described in CLAUDE.md
/// "Output hygiene".
///
/// Detection rule:
///   - depth == 0 + any ```: opens the outer fence (depth = 1)
///   - depth > 0 + bare ```: closes the current fence (depth -= 1)
///   - depth > 0 + labeled ``` (e.g. "```rust"): NESTED opener → trigger!
///
/// This matches CommonMark's rule that a closing code fence must not have an
/// info string, so labeled ``` inside a fence is unambiguously a nested opener.
pub(crate) fn has_nested_fence(text: &str) -> bool {
    let mut depth: usize = 0;
    for line in text.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("```") {
            continue;
        }
        // Everything after the opening backticks (language tag or empty)
        let after = trimmed["```".len()..].trim();
        if depth == 0 {
            // Top-level ``` opens the outer fence.
            depth = 1;
        } else if after.is_empty() {
            // Bare ``` inside a fence closes the current level.
            depth = depth.saturating_sub(1);
        } else {
            // Labeled ``` inside a fence — definitely nested!
            return true;
        }
    }
    false
}

/// Replace inner (nested) triple-backtick fences with tilde-fences (`~~~`) so
/// the Ink renderer never encounters nested backtick blocks.
///
/// Only the inner fences are touched; the outermost opener/closer are
/// preserved so code-block formatting survives intact.
pub(crate) fn sanitize_nested_fences(text: String) -> String {
    let mut result = String::with_capacity(text.len());
    let mut depth: usize = 0;

    for line in text.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("```") {
            result.push_str(line);
            result.push('\n');
            continue;
        }

        // Everything after the opening backticks (language tag or empty)
        let after = trimmed["```".len()..].trim();

        if depth == 0 {
            // Outermost opener — keep as-is.
            result.push_str(line);
            result.push('\n');
            depth = 1;
        } else if after.is_empty() {
            // Bare ``` at depth > 0
            if depth == 1 {
                // Outermost closer — keep as-is.
                result.push_str(line);
                result.push('\n');
                depth = 0;
            } else {
                // Inner closer — replace with tilde fence.
                let replaced = line.replacen("```", "~~~", 1);
                result.push_str(&replaced);
                result.push('\n');
                depth -= 1;
            }
        } else {
            // Labeled ``` at depth > 0 — inner opener, replace with ~~~.
            let replaced = line.replacen("```", "~~~", 1);
            result.push_str(&replaced);
            result.push('\n');
            depth += 1;
        }
    }

    // Preserve absence of trailing newline if original had none.
    if !text.ends_with('\n') && result.ends_with('\n') {
        result.pop();
    }

    result
}

// ============================================================================
// Secret redaction
// ============================================================================

/// Common secret-shaped patterns matched against whitespace-delimited tokens.
///
/// Conservative — false negatives are safer than false positives when
/// rewriting assistant output. Ordered most-specific first.
static SECRET_PATTERNS: &[&str] = &[
    // Anthropic / OpenAI style: sk-<alphanum>
    "sk-",
    // GitHub PATs
    "ghp_",
    "gho_",
    "github_pat_",
    // AWS access key ID prefix
    "AKIA",
    // Bearer tokens
    "Bearer ",
    // Generic API key assignments (common in config snippets)
    "api_key=",
    "apikey=",
    "API_KEY=",
    "APIKEY=",
    // PEM block headers
    "-----BEGIN ",
];

const REDACTION_PLACEHOLDER: &str = "[REDACTED]";

/// Scan `text` for secret-shaped tokens and replace each matching
/// whitespace-delimited word with `[REDACTED]`.
///
/// Returns `(possibly_modified_text, was_modified)`.
pub(crate) fn redact_secrets(text: String) -> (String, bool) {
    let mut result = text.clone();
    let mut modified = false;

    for pattern in SECRET_PATTERNS {
        if result.contains(pattern) {
            result = redact_token_containing(&result, pattern);
            modified = true;
        }
    }

    (result, modified)
}

/// Replace every whitespace-delimited token that contains `pattern` with
/// `[REDACTED]`, preserving the surrounding whitespace.
fn redact_token_containing(text: &str, pattern: &str) -> String {
    // Split preserving whitespace separators so we can stitch back together.
    text.split_inclusive(|c: char| c.is_whitespace())
        .map(|chunk| {
            // `chunk` ends with the whitespace separator (if any). Split the
            // trailing separator off so we can check the word independently.
            let word = chunk.trim_end_matches(|c: char| c.is_whitespace());
            let suffix = &chunk[word.len()..];
            if word.contains(pattern) {
                format!("{REDACTION_PLACEHOLDER}{suffix}")
            } else {
                chunk.to_string()
            }
        })
        .collect()
}

// ============================================================================
// Unit tests (internal helpers)
// ============================================================================

#[cfg(test)]
mod inner_tests {
    use super::*;

    // --- has_nested_fence ---

    #[test]
    fn single_fence_no_nesting() {
        assert!(!has_nested_fence("```rust\nfn main() {}\n```\n"));
    }

    #[test]
    fn two_sequential_fences_no_nesting() {
        let text = "```rust\nfn a() {}\n```\n\n```rust\nfn b() {}\n```\n";
        assert!(!has_nested_fence(text));
    }

    #[test]
    fn nested_labeled_fence_detected() {
        let text = "```markdown\n```rust\nfn main() {}\n```\n```\n";
        assert!(has_nested_fence(text));
    }

    #[test]
    fn no_fence_at_all() {
        assert!(!has_nested_fence("Hello world.\n"));
    }

    // --- sanitize_nested_fences ---

    #[test]
    fn sanitize_removes_nested_labeled_fence() {
        let text = "```markdown\n# Heading\n```rust\nfn main() {}\n```\nend\n```\n".to_string();
        let out = sanitize_nested_fences(text);
        // After sanitization no nested fence must remain.
        assert!(!has_nested_fence(&out));
        // The outer fence markers must be preserved.
        assert!(out.contains("```markdown\n"), "outer opener preserved: {out}");
        // The inner fence must be converted to tildes.
        assert!(out.contains("~~~rust\n"), "inner fence replaced: {out}");
        assert!(out.contains("~~~\n"), "inner closer replaced: {out}");
    }

    #[test]
    fn sanitize_roundtrip_clean_text() {
        let text = "```rust\nfn main() {}\n```\n".to_string();
        let out = sanitize_nested_fences(text.clone());
        // No nesting, output must be identical.
        assert_eq!(out, text);
    }

    // --- redact_secrets ---

    #[test]
    fn sk_token_redacted() {
        let (out, changed) =
            redact_secrets("key: sk-abcdef1234567890abcdef1234567890".to_string());
        assert!(changed);
        assert!(!out.contains("sk-"), "sk- must be gone: {out}");
        assert!(out.contains(REDACTION_PLACEHOLDER));
    }

    #[test]
    fn plain_text_unchanged() {
        let (out, changed) = redact_secrets("Hello world.".to_string());
        assert!(!changed);
        assert_eq!(out, "Hello world.");
    }

    #[test]
    fn github_pat_redacted() {
        let (out, changed) =
            redact_secrets("token: ghp_abcdefghijklmnopqrstuvwxyz123456".to_string());
        assert!(changed);
        assert!(!out.contains("ghp_"), "ghp_ must be gone: {out}");
    }
}
