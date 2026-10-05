//! Keep factory browser work at the affected/targeted tier (cas-c082).
//! Runs before filesystem auto-approval, including without a Cassy root.
use cas_core::hooks::types::HookInput;

fn selected(args: &[String]) -> bool {
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg.starts_with('>') || arg == "2>&1" {
            break;
        }
        let grep = arg.strip_prefix("--grep=").or_else(|| {
            (arg == "--grep" || arg == "-g")
                .then(|| args.get(i + 1).map(String::as_str))
                .flatten()
        });
        if grep.is_some_and(|s| {
            !s.is_empty()
                && s.split('|').all(|id| {
                    let id = id.trim_matches(|c| c == '(' || c == ')' || c == '^' || c == '$');
                    let Some((surface, number)) = id.split_once("-J") else {
                        return false;
                    };
                    !surface.is_empty()
                        && surface.chars().all(|c| c.is_ascii_uppercase())
                        && !number.is_empty()
                        && number.chars().all(|c| c.is_ascii_digit())
                })
        }) {
            return true;
        }
        if matches!(
            arg.as_str(),
            "--config"
                | "-c"
                | "--project"
                | "--output"
                | "--reporter"
                | "--workers"
                | "--timeout"
                | "--grep-invert"
                | "--shard"
        ) {
            i += 2;
            continue;
        }
        if !arg.starts_with('-')
            && (arg.ends_with(".spec.ts") || arg.ends_with(".journey.ts"))
            && !arg.chars().any(|c| matches!(c, '*' | '$' | '`'))
        {
            return true;
        }
        i += 1;
    }
    false
}

// Remove literal stdin bodies before inspecting commands. Documentation and
// fixture source may contain full-suite examples without executing them.
fn shell_commands(command: &str) -> String {
    let marker = regex::Regex::new(r#"^<<-?\s*['"]?([A-Za-z_][A-Za-z0-9_]*)"#)
        .expect("constant heredoc marker");
    let mut delimiter: Option<String> = None;
    let mut clean = String::new();
    for line in command.lines() {
        if let Some(end) = &delimiter {
            if line.trim() == end {
                delimiter = None;
            }
            continue;
        }
        let mut quote = None;
        let mut escaped = false;
        for (offset, ch) in line.char_indices() {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' && quote != Some('\'') {
                escaped = true;
                continue;
            }
            if let Some(q) = quote {
                if ch == q {
                    quote = None;
                }
                continue;
            }
            if matches!(ch, '\'' | '"') {
                quote = Some(ch);
                continue;
            }
            if ch == '<' {
                if let Some(m) = marker.captures(&line[offset..]) {
                    delimiter = Some(m[1].to_string());
                    break;
                }
            }
        }
        clean.push_str(line);
        clean.push(';');
    }
    clean
}

fn unfiltered(command: &str, depth: usize) -> bool {
    if depth > 4 {
        return true;
    }
    for words in super::attribution::split_shell_statements(&shell_commands(command)) {
        if words.first().is_some_and(|w| w.starts_with('#')) {
            continue;
        }
        // Inspect literal shell wrappers; don't turn quoted source text or
        // trace/CLI/version commands into browser suite invocations.
        if let Some(i) = words
            .iter()
            .position(|w| matches!(w.rsplit('/').next(), Some("bash" | "sh" | "zsh")))
        {
            if words.get(i + 1).is_some_and(|w| w == "-c" || w == "-lc")
                && words.get(i + 2).is_some_and(|w| unfiltered(w, depth + 1))
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
            if (verified || args.iter().any(|a| a == "test")) && !selected(args) {
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
                && !selected(&words[i + 1..])
            {
                return true;
            }
        }
    }
    false
}

pub(super) fn denial(input: &HookInput) -> Option<&'static str> {
    if !crate::harness_policy::is_factory_agent(input)
        || crate::harness_policy::is_supervisor(input)
        || input.tool_name.as_deref() != Some("Bash")
    {
        return None;
    }
    let command = input.tool_input.as_ref()?.get("command")?.as_str()?;
    unfiltered(command, 0).then_some(
        "BROWSER TEST TIER: workers and QA run affected journeys. Use scripts/journey-eval.sh <task-artifact-dir> (default affected, four workers), or --affected <base>. For iteration/control use a named spec or canonical --grep journey IDs; preserve original failures. Full/unfiltered browser suites are supervisor-only at epic assembly and in the merge queue; --full is refused here."
    )
}
