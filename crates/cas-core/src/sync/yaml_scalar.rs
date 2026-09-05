//! One place that decides how a value becomes a YAML scalar.
//!
//! Generated frontmatter used to be escaped by three separate hand-rolled
//! `escape_yaml` copies — two skill writers and the spec writer — which had
//! already drifted apart: the spec copy escaped backslashes and the skill
//! copies did not. A description as ordinary as `Use C:\project: inspect` was
//! emitted by the skill writers as
//!
//! ```text
//! description: "Use C:\project: inspect"
//! ```
//!
//! which a YAML reader rejects with `found unknown escape character 'p'`.
//! Quoting is not a string-replacement problem: it has to account for implicit
//! scalars (`true`, `null`, `1.0`, a date), leading indicators (`- ? : # & *
//! !`), surrounding whitespace, quotes and control characters. Every one of
//! those is a rule a hand-rolled escaper has to remember, so this module does
//! not remember them — it asks a real YAML emitter.
//!
//! # Why the output is always one line
//!
//! `serde_yaml` renders a value containing a newline as a block scalar:
//!
//! ```text
//! description: |-
//!   line
//!   break
//! ```
//!
//! That is valid YAML, but frontmatter here is also read by line-oriented
//! consumers, which would see only the first physical line. So a value that
//! cannot be expressed on one line by the emitter is written as an escaped
//! double-quoted scalar instead — produced by `serde_json`, because a JSON
//! string literal is a valid YAML double-quoted scalar. That keeps every path
//! through this module backed by a real serializer rather than a second
//! hand-rolled escaper. The claim is not asserted by inspection: the tests
//! below feed a hostile corpus through a real YAML parser and require every
//! value back byte-identical.

/// Renders `value` as a single-line YAML scalar, suitable for the right-hand
/// side of a `key: ` in generated frontmatter.
///
/// The returned text carries its own quoting when quoting is required, and
/// never contains a newline.
pub fn yaml_scalar(value: &str) -> String {
    // The emitter of record for everything it can express on one line.
    if let Ok(emitted) = serde_yaml::to_string(value) {
        let single = emitted.trim_end_matches('\n');
        if !single.is_empty() && !single.contains('\n') {
            return single.to_string();
        }
    }
    json_string_as_yaml_scalar(value)
}

/// The single-line form used when the YAML emitter would go multi-line.
///
/// This is still a real serializer, not a hand-rolled escaper: a JSON string
/// literal *is* a YAML double-quoted scalar (YAML 1.2 is a superset of JSON,
/// and YAML's double-quoted style accepts the same `\n`, `\t`, `\"`, `\\` and
/// `\uXXXX` escapes JSON emits). `serde_json` already ships in this crate, it
/// never emits a raw newline, and it escapes every control character — so it
/// answers exactly the question this fallback needs answered. The corpus test
/// verifies that claim against a YAML parser rather than asserting it.
fn json_string_as_yaml_scalar(value: &str) -> String {
    // Serializing a `&str` cannot fail: serde_json's error cases are I/O,
    // non-string map keys and unrepresentable numbers, none of which a string
    // can reach. Stating that invariant is honest; swallowing an error into an
    // empty scalar would silently blank a description, which is worse than the
    // bug this module fixes.
    serde_json::to_string(value).expect("serializing a &str to a JSON string is infallible")
}

#[cfg(test)]
mod tests {
    use super::yaml_scalar;

    /// Values that a description, name or hint can plausibly contain, plus the
    /// ones that break naive escaping. Each must survive a real parser.
    const HOSTILE_CORPUS: &[&str] = &[
        // The reported defect.
        "Use C:\\project: inspect",
        // Backslashes with and without other triggers.
        "back\\slash",
        "C:\\Users\\dev\\project",
        "regex \\d+ and \\\\ literal",
        // Quotes.
        "quote \" inside",
        "it's fine",
        "both ' and \" together",
        "\"already quoted\"",
        "'single quoted'",
        // Colons, the original quoting trigger.
        "key: value",
        "ends with colon:",
        "ratio 1:2:3",
        // YAML indicators in leading position.
        "- leading dash",
        "? leading question",
        ": leading colon",
        "# leading hash",
        "& anchor",
        "* alias",
        "! tag",
        "| pipe",
        "> gt",
        "% directive",
        "@ at",
        "` backtick",
        "[ bracket",
        "{ brace",
        // Implicit scalars that must stay strings.
        "true",
        "false",
        "null",
        "~",
        "123",
        "1.0",
        "0x1f",
        "2026-09-04",
        "no",
        "on",
        // Whitespace.
        " leading space",
        "trailing space ",
        "  both  ",
        "\ttab lead",
        // Newlines and control characters.
        "line\nbreak",
        "carriage\rreturn",
        "bell\u{7}char",
        // Unicode and emptiness.
        "unicode — em dash, café, 日本語",
        "emoji 🎯 inside",
        "",
        // Something ordinary, to prove the common case stays readable.
        "Use when an agent must post a Slack message through the hub.",
    ];

    /// The contract: what the emitter writes, a real parser reads back
    /// unchanged, on one line.
    #[test]
    fn every_hostile_value_round_trips_through_a_real_parser() {
        for value in HOSTILE_CORPUS {
            let scalar = yaml_scalar(value);
            assert!(
                !scalar.contains('\n'),
                "frontmatter must stay on one line, got {scalar:?} for {value:?}"
            );

            let document = format!("description: {scalar}\n");
            let parsed: serde_yaml::Value = serde_yaml::from_str(&document)
                .unwrap_or_else(|e| panic!("emitted invalid YAML for {value:?}: {e}\n{document}"));
            let read_back = parsed
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| panic!("description was not a string for {value:?}: {document}"));

            assert_eq!(
                read_back, *value,
                "value changed on the way through YAML: {document}"
            );
        }
    }

    /// The reason this module exists, kept executable rather than described.
    /// The old hand-rolled escaper is reproduced verbatim from the skill
    /// writers; feeding it the same corpus must fail, so nobody can quietly
    /// reintroduce it as "equivalent".
    #[test]
    fn the_replaced_hand_rolled_escaper_fails_the_same_corpus() {
        fn legacy_escape_yaml(s: &str) -> String {
            if s.contains(':') || s.contains('#') || s.contains('\n') || s.starts_with(' ') {
                format!("\"{}\"", s.replace('\"', "\\\"").replace('\n', "\\n"))
            } else {
                s.to_string()
            }
        }

        let mut failures = Vec::new();
        for value in HOSTILE_CORPUS {
            let document = format!("description: {}\n", legacy_escape_yaml(value));
            let survived = serde_yaml::from_str::<serde_yaml::Value>(&document)
                .ok()
                .and_then(|parsed| {
                    parsed
                        .get("description")
                        .and_then(|v| v.as_str())
                        .map(|s| s == *value)
                })
                .unwrap_or(false);
            if !survived {
                failures.push(*value);
            }
        }

        assert!(
            failures.contains(&"Use C:\\project: inspect"),
            "the reported defect must still reproduce against the old escaper"
        );
        assert!(
            failures.len() >= 10,
            "the old escaper is expected to mishandle much of this corpus, \
             it failed only on: {failures:?}"
        );
    }

    /// Ordinary text must not acquire noise: a description that needs no
    /// quoting is written plainly, so generated files stay readable.
    #[test]
    fn ordinary_text_is_not_quoted() {
        assert_eq!(yaml_scalar("Use when posting a message"), "Use when posting a message");
        assert_eq!(yaml_scalar("plain"), "plain");
    }

    /// A newline takes the escaped single-line path rather than a block scalar.
    #[test]
    fn a_newline_stays_on_one_line() {
        let scalar = yaml_scalar("line\nbreak");
        assert_eq!(scalar, "\"line\\nbreak\"");
        assert!(!scalar.contains('\n'));
    }

    /// The fallback's premise, stated as a test rather than a comment: what
    /// serde_json writes for a hostile value is accepted by a YAML parser and
    /// read back unchanged. If that ever stops holding, this fails here rather
    /// than in generated files.
    #[test]
    fn json_string_literals_are_valid_yaml_scalars() {
        for value in HOSTILE_CORPUS {
            let scalar = super::json_string_as_yaml_scalar(value);
            assert!(!scalar.contains('\n'), "{value:?} -> {scalar:?}");

            let document = format!("description: {scalar}\n");
            let parsed: serde_yaml::Value = serde_yaml::from_str(&document).unwrap_or_else(|e| {
                panic!("serde_json output is not valid YAML for {value:?}: {e}\n{document}")
            });
            assert_eq!(
                parsed.get("description").and_then(|v| v.as_str()),
                Some(*value),
                "value changed through the JSON-literal fallback: {document}"
            );
        }
    }
}
