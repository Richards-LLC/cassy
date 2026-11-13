//! Keep factory browser work at the affected/targeted tier (cas-c082).
//! Runs before filesystem auto-approval, including without a Cassy root.
use cas_core::hooks::types::HookInput;

/// Playwright test options whose next argument is their value.
const VALUE_OPTIONS: &[&str] = &[
    "--config",
    "-c",
    "--project",
    "--output",
    "--reporter",
    "--workers",
    "--timeout",
    "--grep",
    "-g",
    "--grep-invert",
    "--shard",
    "--retries",
    "-j",
    "--trace",
    "--repeat-each",
    "--max-failures",
    "--browser",
    "--global-timeout",
    "--tsconfig",
];

/// Boolean options compatible with collection-only runs. Interactive --ui is
/// intentionally excluded: it launches a server even when --list is present.
const BOOLEAN_OPTIONS: &[&str] = &[
    "--fail-on-flaky-tests",
    "--forbid-only",
    "--fully-parallel",
    "--headed",
    "--ignore-snapshots",
    "--last-failed",
    "--no-deps",
    "--pass-with-no-tests",
    "--quiet",
    "-x",
];

/// Playwright options whose value is optional (`--shuffle[=seed]`). A numeric
/// next word is the value, never a file filter (GH #1876); anything else is
/// left for spec selection, so `--shuffle tests/a.spec.ts` still names a spec.
const OPTIONAL_VALUE_OPTIONS: &[&str] = &["--shuffle"];

fn optional_value_width(arg: &str, next: Option<&String>) -> Option<usize> {
    OPTIONAL_VALUE_OPTIONS.contains(&arg).then(|| {
        1 + usize::from(
            next.is_some_and(|v| !v.is_empty() && v.bytes().all(|c| c.is_ascii_digit())),
        )
    })
}

/// `test --list` only collects: no browser, server or test runs (GH #1852).
/// Every option must be understood, because an option value can itself be
/// `--list` (`--grep-invert --list` runs the suite) and `--` turns later
/// words into file filters. Positional words only narrow the listing.
fn collection_only(args: &[String]) -> bool {
    let mut list = false;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "--list" {
            list = true;
        } else if BOOLEAN_OPTIONS.contains(&arg) {
            // No value to consume; the next token may still be --list.
        } else if VALUE_OPTIONS.contains(&arg) {
            i += 1;
        } else if let Some(width) = optional_value_width(arg, args.get(i + 1)) {
            i += width - 1;
        } else if arg.starts_with('-')
            && !arg.split_once('=').is_some_and(|(name, _)| {
                name.starts_with("--")
                    && (VALUE_OPTIONS.contains(&name) || OPTIONAL_VALUE_OPTIONS.contains(&name))
            })
        {
            return false;
        }
        i += 1;
    }
    list
}

fn selected(args: &[String], culprit: &mut Option<String>) -> bool {
    if args
        .iter()
        .any(|arg| arg == "--full" || arg.starts_with("--full="))
    {
        return false;
    }
    let mut i = 0;
    let mut targeted = false;
    while i < args.len() {
        let arg = &args[i];
        let grep = arg.strip_prefix("--grep=").or_else(|| {
            (arg == "--grep" || arg == "-g")
                .then(|| args.get(i + 1).map(String::as_str))
                .flatten()
        });
        if let Some(grep) = grep {
            targeted |= !grep.is_empty()
                && grep.split('|').all(|id| {
                    let id = id.trim_matches(|c| c == '(' || c == ')' || c == '^' || c == '$');
                    let Some((surface, number)) = id.split_once("-J") else {
                        return false;
                    };
                    !surface.is_empty()
                        && surface.chars().all(|c| c.is_ascii_uppercase())
                        && !number.is_empty()
                        && number.chars().all(|c| c.is_ascii_digit())
                });
            i += if arg == "--grep" || arg == "-g" { 2 } else { 1 };
            continue;
        }
        if VALUE_OPTIONS.contains(&arg.as_str()) {
            i += 2;
            continue;
        }
        if let Some(width) = optional_value_width(arg, args.get(i + 1)) {
            i += width;
            continue;
        }
        if !arg.starts_with('-') {
            let mut spec = arg.as_str();
            // Playwright accepts file:line and file:line:column selectors.
            for _ in 0..2 {
                let Some((file, _)) = spec.rsplit_once(':').filter(|(_, coordinate)| {
                    !coordinate.is_empty() && coordinate.bytes().all(|c| c.is_ascii_digit())
                }) else {
                    break;
                };
                spec = file;
            }
            let filename = std::path::Path::new(spec)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if arg.starts_with('.')
                || filename.starts_with('.')
                || arg.chars().any(|c| {
                    matches!(
                        c,
                        '\\' | '+'
                            | '('
                            | ')'
                            | '|'
                            | '{'
                            | '}'
                            | '^'
                            | '*'
                            | '?'
                            | '['
                            | ']'
                            | '$'
                            | '`'
                    )
                })
            {
                *culprit = Some(arg.clone());
                return false;
            }
            let Some(stem) = [
                ".spec.ts",
                ".spec.mjs",
                ".spec.cjs",
                ".spec.js",
                ".spec.tsx",
                ".journey.ts",
            ]
            .iter()
            .find_map(|suffix| filename.strip_suffix(suffix)) else {
                // Playwright ORs file filters, so a plain word can widen the run too.
                *culprit = Some(arg.clone());
                return false;
            };
            if !stem.chars().any(char::is_alphanumeric) {
                *culprit = Some(arg.clone());
                return false;
            }
            targeted = true;
        }
        i += 1;
    }
    targeted
}

fn unfiltered(command: &str, depth: usize, culprit: &mut Option<String>) -> bool {
    if depth > 4 {
        return true;
    }
    for words in super::pre_tool::shell_statement_arguments(command) {
        if words.first().is_some_and(|w| w.starts_with('#')) {
            continue;
        }
        let Some(executable) = super::pre_tool::executable_word_index(&words) else {
            continue;
        };
        let name = words[executable].rsplit('/').next().unwrap_or("");
        if matches!(
            name,
            "cat" | "printf" | "echo" | "rg" | "grep" | "sed" | "head" | "tail"
        ) {
            continue;
        }
        // Inspect literal shell wrappers; don't turn quoted source text or
        // trace/CLI/version commands into browser suite invocations.
        if let Some(i) = words
            .iter()
            .position(|w| matches!(w.rsplit('/').next(), Some("bash" | "sh" | "zsh")))
        {
            if words.get(i + 1).is_some_and(|w| w == "-c" || w == "-lc")
                && words
                    .get(i + 2)
                    .is_some_and(|w| unfiltered(w, depth + 1, culprit))
            {
                return true;
            }
        }
        if words.iter().any(|w| {
            matches!(
                w.rsplit('/').next(),
                Some("journey-eval.sh" | "journey-receipt.py")
            )
        }) {
            if words
                .iter()
                .any(|w| w == "--full" || w.starts_with("--full="))
            {
                return true;
            }
            continue;
        }
        let runner = words.iter().position(|w| {
            w == "playwright"
                || w.ends_with("/@playwright/test/cli.js")
                || w.ends_with("/playwright/cli.js")
        });
        if let Some(i) = runner {
            let args = &words[i + 1..];
            let verified = words[..i]
                .iter()
                .any(|w| w.ends_with("run-verified-tests.mjs"));
            // Help as the only argument runs nothing. Anywhere later it can be an
            // option's value: `test --grep-invert --help` runs the whole suite.
            let help_only = !verified
                && matches!(
                    args,
                    [subcommand, help] if subcommand == "test" && (help == "--help" || help == "-h")
                );
            let collection = !verified
                && args.first().is_some_and(|arg| arg == "test")
                && collection_only(&args[1..]);
            let needs_selection =
                !help_only && !collection && (verified || args.iter().any(|a| a == "test"));
            // The direct CLI consumes its subcommand; the verified runner adds
            // that subcommand itself, so all of its args are filters/options.
            let args = if !verified && args.first().is_some_and(|arg| arg == "test") {
                &args[1..]
            } else {
                args
            };
            if needs_selection && !selected(args, culprit) {
                return true;
            }
        }
        if let Some(i) = words.iter().position(|w| {
            matches!(
                w.as_str(),
                "journeys" | "test:journeys" | "journeys:real-hub"
            )
        }) {
            if words[..i].iter().any(|w| w == "run" || w == "run-script")
                && !selected(&words[i + 1..], culprit)
            {
                return true;
            }
        }
    }
    false
}

pub(super) fn denial(input: &HookInput) -> Option<String> {
    if !crate::harness_policy::is_factory_agent(input)
        || crate::harness_policy::is_supervisor(input)
        || input.tool_name.as_deref() != Some("Bash")
    {
        return None;
    }
    let command = input.tool_input.as_ref()?.get("command")?.as_str()?;
    let mut culprit = None;
    unfiltered(command, 0, &mut culprit).then(|| {
        let mut reason = String::from(
        "BROWSER TEST TIER: authoring follows cas-playwright-tests; workers run touched vitest files and typecheck; journeys run in batch QA. Iterate on a named spec (tests/example.spec.cjs --workers=1) or canonical --grep journey IDs, preserving original failures, or scripts/journey-eval.sh <task-artifact-dir> (default affected). The full suite runs once on the landing tree: in the merge queue, else CI, else one supervisor run per epic. --full is refused here.",
        );
        if let Some(token) = culprit {
            reason.push_str(&format!(" Could not read `{token}` as a named spec file or a known option."));
        }
        reason
    })
}

#[cfg(test)]
mod tests {
    /// GH #1876: a refusal carries the token it could not read.
    #[test]
    fn a_refused_run_reports_the_token_it_could_not_read() {
        let mut culprit = None;
        assert!(super::unfiltered(
            "npx playwright test qa/a.spec.ts widen",
            0,
            &mut culprit
        ));
        assert_eq!(culprit.as_deref(), Some("widen"));
    }
}
