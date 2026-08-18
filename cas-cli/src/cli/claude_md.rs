//! `cas claude-md` — Evaluate and optimize CLAUDE.md files for token efficiency.
//!
//! Analyzes CLAUDE.md files against best practices and provides actionable
//! optimization recommendations. Works without a Cassy project (user-level).

use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::Args;

use crate::cli::Cli;
use crate::ui::components::Formatter;
use crate::ui::theme::ActiveTheme;

/// Token targets from community best practices and Anthropic guidance.
const TARGET_LINES: usize = 100;
const WARN_LINES: usize = 200;
const TARGET_TOKENS: usize = 1500;
const WARN_TOKENS: usize = 2500;

/// Lines that add no value — Claude already does these by default.
const OBVIOUS_PATTERNS: &[&str] = &[
    "write clean code",
    "use meaningful variable names",
    "follow best practices",
    "write readable code",
    "keep functions small",
    "use descriptive names",
    "handle errors appropriately",
    "add appropriate error handling",
    "write maintainable code",
    "use proper indentation",
    "follow coding standards",
    "write well-documented code",
    "use consistent naming",
    "keep it simple",
    "don't repeat yourself",
    "follow dry principles",
    "write tests for your code",
    "use version control",
    "follow solid principles",
    "write modular code",
];

/// Style/lint rules that should be in tooling config, not CLAUDE.md.
const LINT_PATTERNS: &[&str] = &[
    "use 2 spaces for indentation",
    "use 4 spaces for indentation",
    "use tabs for indentation",
    "max line length",
    "trailing comma",
    "semicolons at end",
    "no trailing whitespace",
    "use single quotes",
    "use double quotes",
    "prefer const over let",
    "no unused variables",
    "no unused imports",
];

#[derive(Args, Debug, Clone)]
pub struct ClaudeMdArgs {
    /// Path to CLAUDE.md file (default: search current directory hierarchy)
    #[arg()]
    pub path: Option<PathBuf>,

    /// Scan the entire CLAUDE.md hierarchy (global, project, subdirectories)
    #[arg(long)]
    pub hierarchy: bool,
}

pub fn execute(args: &ClaudeMdArgs, cli: &Cli) -> anyhow::Result<()> {
    let files = if args.hierarchy {
        discover_hierarchy(&std::env::current_dir()?)
    } else if let Some(path) = &args.path {
        if path.exists() {
            vec![path.clone()]
        } else {
            anyhow::bail!("File not found: {}", path.display());
        }
    } else {
        let cwd = std::env::current_dir()?;
        discover_nearest(&cwd)
    };

    if files.is_empty() {
        anyhow::bail!(
            "No CLAUDE.md found. Search paths: ./CLAUDE.md, ./.claude/CLAUDE.md, ~/CLAUDE.md, ~/.claude/CLAUDE.md"
        );
    }

    let mut all_reports = Vec::new();
    for file in &files {
        let content =
            std::fs::read_to_string(file).with_context(|| format!("Reading {}", file.display()))?;
        let report = analyze(&content, file);
        all_reports.push(report);
    }

    if cli.json {
        output_json(&all_reports)
    } else {
        output_pretty(&all_reports, cli)
    }
}

// ─── Discovery ───────────────────────────────────────────────────────────────

/// Find the nearest CLAUDE.md (project root, then .claude/, then global).
fn discover_nearest(cwd: &Path) -> Vec<PathBuf> {
    let candidates = [
        cwd.join("CLAUDE.md"),
        cwd.join(".claude/CLAUDE.md"),
    ];
    for c in &candidates {
        if c.exists() {
            return vec![c.clone()];
        }
    }
    // Walk up to find project root CLAUDE.md
    let mut dir = cwd.parent();
    while let Some(d) = dir {
        let candidate = d.join("CLAUDE.md");
        if candidate.exists() {
            return vec![candidate];
        }
        dir = d.parent();
    }
    // Global
    if let Some(home) = dirs::home_dir() {
        let global = home.join(".claude/CLAUDE.md");
        if global.exists() {
            return vec![global];
        }
        let global_root = home.join("CLAUDE.md");
        if global_root.exists() {
            return vec![global_root];
        }
    }
    vec![]
}

/// Discover the full CLAUDE.md hierarchy for --hierarchy mode.
fn discover_hierarchy(cwd: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();

    // Global
    if let Some(home) = dirs::home_dir() {
        let global = home.join("CLAUDE.md");
        if global.exists() {
            files.push(global);
        }
        let global_claude = home.join(".claude/CLAUDE.md");
        if global_claude.exists() {
            files.push(global_claude);
        }
    }

    // Walk up from cwd to find project root and ancestors
    let mut ancestors = Vec::new();
    let mut dir = Some(cwd.to_path_buf());
    while let Some(d) = dir {
        let candidate = d.join("CLAUDE.md");
        if candidate.exists() && !files.contains(&candidate) {
            ancestors.push(candidate.clone());
        }
        let candidate_inner = d.join(".claude/CLAUDE.md");
        if candidate_inner.exists() && !files.contains(&candidate_inner) {
            ancestors.push(candidate_inner.clone());
        }
        dir = d.parent().map(Path::to_path_buf);
    }
    ancestors.reverse();
    files.extend(ancestors);

    // Subdirectory CLAUDE.md files (one level deep scan)
    if let Ok(entries) = std::fs::read_dir(cwd) {
        for entry in entries.flatten() {
            if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                let sub = entry.path().join("CLAUDE.md");
                if sub.exists() && !files.contains(&sub) {
                    files.push(sub);
                }
            }
        }
    }

    files
}

// ─── Analysis ────────────────────────────────────────────────────────────────

struct Report {
    path: PathBuf,
    line_count: usize,
    token_estimate: usize,
    sections: Vec<Section>,
    findings: Vec<Finding>,
    score: u8,
    at_imports: Vec<AtImport>,
}

struct Section {
    name: String,
    start_line: usize,
    line_count: usize,
    token_estimate: usize,
}

struct Finding {
    severity: Severity,
    category: &'static str,
    message: String,
    line: Option<usize>,
    suggestion: Option<String>,
}

struct AtImport {
    path: String,
    line: usize,
}

#[derive(Clone, Copy)]
enum Severity {
    Info,
    Warning,
    Error,
}

fn analyze(content: &str, path: &Path) -> Report {
    let lines: Vec<&str> = content.lines().collect();
    let line_count = lines.len();
    let token_estimate = estimate_tokens(content);
    let sections = extract_sections(&lines);
    let at_imports = extract_at_imports(&lines);
    let mut findings = Vec::new();

    // ── Size checks ──────────────────────────────────────────────────────

    if line_count > WARN_LINES {
        findings.push(Finding {
            severity: Severity::Error,
            category: "size",
            message: format!(
                "{line_count} lines (target: <{TARGET_LINES}, max: <{WARN_LINES})"
            ),
            line: None,
            suggestion: Some(
                "Extract large sections (architecture, tables) to linked docs and breadcrumb from CLAUDE.md".into(),
            ),
        });
    } else if line_count > TARGET_LINES {
        findings.push(Finding {
            severity: Severity::Warning,
            category: "size",
            message: format!("{line_count} lines (target: <{TARGET_LINES})"),
            line: None,
            suggestion: Some("Review each line: would removing it cause Claude to make mistakes? If not, cut it.".into()),
        });
    }

    if token_estimate > WARN_TOKENS {
        findings.push(Finding {
            severity: Severity::Error,
            category: "tokens",
            message: format!(
                "~{token_estimate} tokens (target: <{TARGET_TOKENS}, max: <{WARN_TOKENS})"
            ),
            line: None,
            suggestion: Some(
                "Adherence degrades linearly with instruction count. Prioritize the rules Claude would get wrong without.".into(),
            ),
        });
    } else if token_estimate > TARGET_TOKENS {
        findings.push(Finding {
            severity: Severity::Warning,
            category: "tokens",
            message: format!("~{token_estimate} tokens (target: <{TARGET_TOKENS})"),
            line: None,
            suggestion: None,
        });
    }

    // ── Large sections ───────────────────────────────────────────────────

    for section in &sections {
        if section.line_count > 30 {
            findings.push(Finding {
                severity: Severity::Warning,
                category: "section-size",
                message: format!(
                    "Section '{}' is {} lines (~{} tokens)",
                    section.name, section.line_count, section.token_estimate
                ),
                line: Some(section.start_line),
                suggestion: Some(format!(
                    "Extract to a separate doc file and add breadcrumb: '-> See docs/{}.md'",
                    section.name.to_lowercase().replace(' ', "-")
                )),
            });
        }
    }

    // ── Markdown tables ──────────────────────────────────────────────────

    let table_lines = count_table_lines(&lines);
    if table_lines > 10 {
        findings.push(Finding {
            severity: Severity::Warning,
            category: "tables",
            message: format!(
                "{table_lines} lines of markdown tables — tables are token-expensive"
            ),
            line: None,
            suggestion: Some(
                "Move tables to linked docs. Tables use ~2x tokens vs plain lists.".into(),
            ),
        });
    }

    // ── @imports ─────────────────────────────────────────────────────────

    for imp in &at_imports {
        findings.push(Finding {
            severity: Severity::Warning,
            category: "at-import",
            message: format!("@-import '{}' inlines entire file eagerly every session", imp.path),
            line: Some(imp.line),
            suggestion: Some(format!(
                "Replace with path reference: 'See {}' — only loaded when agent reads it",
                imp.path
            )),
        });
    }

    // ── Obvious/redundant instructions ───────────────────────────────────

    for (i, line) in lines.iter().enumerate() {
        let lower = line.to_lowercase();
        for pattern in OBVIOUS_PATTERNS {
            if lower.contains(pattern) {
                findings.push(Finding {
                    severity: Severity::Warning,
                    category: "obvious",
                    message: format!("Redundant instruction — Claude does this by default"),
                    line: Some(i + 1),
                    suggestion: Some(format!("Remove: '{}'", line.trim())),
                });
                break;
            }
        }
    }

    // ── Lint/style rules that belong in tooling ──────────────────────────

    for (i, line) in lines.iter().enumerate() {
        let lower = line.to_lowercase();
        for pattern in LINT_PATTERNS {
            if lower.contains(pattern) {
                findings.push(Finding {
                    severity: Severity::Info,
                    category: "lint-rule",
                    message: "Style rule that should be in linter/formatter config".into(),
                    line: Some(i + 1),
                    suggestion: Some(format!(
                        "Move to .eslintrc / biome.json / rustfmt.toml / .editorconfig instead: '{}'",
                        line.trim()
                    )),
                });
                break;
            }
        }
    }

    // ── Prohibitions without alternatives ────────────────────────────────

    for (i, line) in lines.iter().enumerate() {
        let lower = line.to_lowercase();
        let is_prohibition = lower.contains("don't use")
            || lower.contains("do not use")
            || lower.contains("never use")
            || lower.contains("avoid using");
        let has_alternative = lower.contains("instead")
            || lower.contains("prefer")
            || lower.contains("use … instead")
            || lower.contains("use...instead");
        if is_prohibition && !has_alternative {
            findings.push(Finding {
                severity: Severity::Warning,
                category: "no-alternative",
                message: "Prohibition without alternative — Claude may get stuck".into(),
                line: Some(i + 1),
                suggestion: Some(format!(
                    "Add an alternative: '{}; prefer X instead'",
                    line.trim()
                )),
            });
        }
    }

    // ── Large code blocks ────────────────────────────────────────────────

    let mut in_code_block = false;
    let mut code_block_start = 0;
    let mut code_block_lines = 0;
    for (i, line) in lines.iter().enumerate() {
        if line.trim_start().starts_with("```") {
            if in_code_block {
                // Closing
                if code_block_lines > 15 {
                    findings.push(Finding {
                        severity: Severity::Warning,
                        category: "code-block",
                        message: format!(
                            "Code block is {code_block_lines} lines — consider condensing"
                        ),
                        line: Some(code_block_start + 1),
                        suggestion: Some(
                            "Keep only the essential commands. Move full examples to linked docs."
                                .into(),
                        ),
                    });
                }
                in_code_block = false;
                code_block_lines = 0;
            } else {
                in_code_block = true;
                code_block_start = i;
                code_block_lines = 0;
            }
        } else if in_code_block {
            code_block_lines += 1;
        }
    }

    // ── Dense prose paragraphs ───────────────────────────────────────────

    let mut consecutive_prose = 0;
    let mut prose_start = 0;
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let is_prose = !trimmed.is_empty()
            && !trimmed.starts_with('#')
            && !trimmed.starts_with('-')
            && !trimmed.starts_with('*')
            && !trimmed.starts_with('|')
            && !trimmed.starts_with("```")
            && !trimmed.starts_with('>');
        if is_prose {
            if consecutive_prose == 0 {
                prose_start = i;
            }
            consecutive_prose += 1;
        } else {
            if consecutive_prose > 5 {
                findings.push(Finding {
                    severity: Severity::Info,
                    category: "dense-prose",
                    message: format!(
                        "{consecutive_prose} consecutive prose lines — low scanability for LLMs"
                    ),
                    line: Some(prose_start + 1),
                    suggestion: Some(
                        "Convert to bullet points or extract to linked doc. Lists are easier to follow.".into(),
                    ),
                });
            }
            consecutive_prose = 0;
        }
    }

    // ── Missing structure ────────────────────────────────────────────────

    let has_commands = sections.iter().any(|s| {
        let lower = s.name.to_lowercase();
        lower.contains("build") || lower.contains("test") || lower.contains("command")
    });
    if !has_commands && line_count > 10 {
        findings.push(Finding {
            severity: Severity::Info,
            category: "structure",
            message: "No 'Build', 'Test', or 'Commands' section found".into(),
            line: None,
            suggestion: Some(
                "Add a Commands section with build/test/lint commands Claude can't infer.".into(),
            ),
        });
    }

    // ── Duplicate content detection (for hierarchy mode) ─────────────────

    // This is checked at the reporting level when multiple files exist

    // ── Score ────────────────────────────────────────────────────────────

    let score = calculate_score(line_count, token_estimate, &findings);

    Report {
        path: path.to_path_buf(),
        line_count,
        token_estimate,
        sections,
        findings,
        score,
        at_imports,
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Rough token estimation: ~1 token per 4 characters for English/code mixed content.
/// Adjusted up slightly for markdown syntax overhead.
fn estimate_tokens(content: &str) -> usize {
    // Use character-based estimation: ~3.5 chars per token for mixed content
    let chars = content.len();
    ((chars as f64 / 3.5).ceil() as usize).max(1)
}

fn extract_sections(lines: &[&str]) -> Vec<Section> {
    let mut sections = Vec::new();
    let mut current_name: Option<String> = None;
    let mut current_start = 0;
    let mut current_content = String::new();

    for (i, line) in lines.iter().enumerate() {
        if line.starts_with('#') {
            // Close previous section
            if let Some(name) = current_name.take() {
                let section_lines = i - current_start;
                sections.push(Section {
                    name,
                    start_line: current_start + 1,
                    line_count: section_lines,
                    token_estimate: estimate_tokens(&current_content),
                });
            }
            current_name = Some(line.trim_start_matches('#').trim().to_string());
            current_start = i;
            current_content.clear();
        } else {
            current_content.push_str(line);
            current_content.push('\n');
        }
    }

    // Close last section
    if let Some(name) = current_name {
        let section_lines = lines.len() - current_start;
        sections.push(Section {
            name,
            start_line: current_start + 1,
            line_count: section_lines,
            token_estimate: estimate_tokens(&current_content),
        });
    }

    sections
}

fn extract_at_imports(lines: &[&str]) -> Vec<AtImport> {
    let mut imports = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        // Match @path patterns that look like file references
        // e.g., @README.md, @docs/architecture.md, @~/my-config.md
        let trimmed = line.trim();
        for word in trimmed.split_whitespace() {
            if word.starts_with('@')
                && word.len() > 2
                && (word.contains('.') || word.contains('/'))
                && !word.starts_with("@{")
                && !word.contains("@anthropic")
                && !word.contains("@claude")
            {
                imports.push(AtImport {
                    path: word.to_string(),
                    line: i + 1,
                });
            }
        }
    }
    imports
}

fn count_table_lines(lines: &[&str]) -> usize {
    lines
        .iter()
        .filter(|l| {
            let trimmed = l.trim();
            trimmed.starts_with('|') && trimmed.ends_with('|')
        })
        .count()
}

fn calculate_score(line_count: usize, token_estimate: usize, findings: &[Finding]) -> u8 {
    let mut score: i32 = 100;

    // Size penalties
    if line_count > WARN_LINES {
        score -= 25;
    } else if line_count > TARGET_LINES {
        score -= 10;
    }

    if token_estimate > WARN_TOKENS {
        score -= 25;
    } else if token_estimate > TARGET_TOKENS {
        score -= 10;
    }

    // Finding penalties
    for f in findings {
        match f.severity {
            Severity::Error => score -= 10,
            Severity::Warning => score -= 3,
            Severity::Info => score -= 1,
        }
    }

    score.clamp(0, 100) as u8
}

// ─── Output ──────────────────────────────────────────────────────────────────

fn output_json(reports: &[Report]) -> anyhow::Result<()> {
    let json_reports: Vec<_> = reports
        .iter()
        .map(|r| {
            let findings: Vec<_> = r
                .findings
                .iter()
                .map(|f| {
                    let mut obj = serde_json::json!({
                        "severity": match f.severity {
                            Severity::Error => "error",
                            Severity::Warning => "warning",
                            Severity::Info => "info",
                        },
                        "category": f.category,
                        "message": f.message,
                    });
                    if let Some(line) = f.line {
                        obj["line"] = serde_json::json!(line);
                    }
                    if let Some(suggestion) = &f.suggestion {
                        obj["suggestion"] = serde_json::json!(suggestion);
                    }
                    obj
                })
                .collect();

            let sections: Vec<_> = r
                .sections
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "name": s.name,
                        "start_line": s.start_line,
                        "lines": s.line_count,
                        "tokens": s.token_estimate,
                    })
                })
                .collect();

            serde_json::json!({
                "path": r.path.display().to_string(),
                "lines": r.line_count,
                "tokens": r.token_estimate,
                "score": r.score,
                "sections": sections,
                "findings": findings,
                "at_imports": r.at_imports.iter().map(|i| {
                    serde_json::json!({ "path": i.path, "line": i.line })
                }).collect::<Vec<_>>(),
            })
        })
        .collect();

    let output = if json_reports.len() == 1 {
        json_reports.into_iter().next().unwrap()
    } else {
        let total_tokens: usize = reports.iter().map(|r| r.token_estimate).sum();
        serde_json::json!({
            "files": json_reports,
            "total_tokens": total_tokens,
        })
    };

    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn output_pretty(reports: &[Report], _cli: &Cli) -> anyhow::Result<()> {
    let theme = ActiveTheme::default();
    let mut out = std::io::stdout();
    let mut fmt = Formatter::stdout(&mut out, theme);

    let total_tokens: usize = reports.iter().map(|r| r.token_estimate).sum();
    let total_lines: usize = reports.iter().map(|r| r.line_count).sum();

    if reports.len() > 1 {
        fmt.subheading("CLAUDE.md hierarchy analysis")?;
        fmt.field("Files", &reports.len().to_string())?;
        fmt.field(
            "Total",
            &format!("{total_lines} lines, ~{total_tokens} tokens (all eagerly loaded)"),
        )?;
        fmt.separator()?;
    }

    for report in reports {
        print_report(report, &mut fmt)?;
        if reports.len() > 1 {
            fmt.separator()?;
        }
    }

    // Summary
    if reports.len() > 1 {
        fmt.newline()?;
        if total_tokens > WARN_TOKENS {
            fmt.error(&format!(
                "Combined token load: ~{total_tokens} — exceeds {WARN_TOKENS} target across hierarchy"
            ))?;
        } else if total_tokens > TARGET_TOKENS {
            fmt.warning(&format!(
                "Combined token load: ~{total_tokens} — approaching {WARN_TOKENS} limit"
            ))?;
        } else {
            fmt.success(&format!(
                "Combined token load: ~{total_tokens} — within target"
            ))?;
        }
    }

    Ok(())
}

fn print_report(report: &Report, fmt: &mut Formatter) -> std::io::Result<()> {
    fmt.newline()?;
    fmt.subheading(&report.path.display().to_string())?;

    // Score
    let score_label = match report.score {
        90..=100 => "Excellent",
        75..=89 => "Good",
        50..=74 => "Needs work",
        _ => "Poor",
    };

    fmt.field("Score", &format!("{}/100 ({})", report.score, score_label))?;
    fmt.field(
        "Size",
        &format!(
            "{} lines, ~{} tokens",
            report.line_count, report.token_estimate
        ),
    )?;

    // Section breakdown
    if !report.sections.is_empty() {
        fmt.newline()?;
        fmt.write_bold("Sections:")?;
        fmt.newline()?;
        for section in &report.sections {
            let marker = if section.line_count > 30 {
                "!"
            } else {
                " "
            };
            fmt.write_raw(&format!(
                "  {marker} {:<30} {:>3} lines  ~{:>4} tokens",
                section.name, section.line_count, section.token_estimate
            ))?;
            fmt.newline()?;
        }
    }

    // Findings
    let errors: Vec<_> = report
        .findings
        .iter()
        .filter(|f| matches!(f.severity, Severity::Error))
        .collect();
    let warnings: Vec<_> = report
        .findings
        .iter()
        .filter(|f| matches!(f.severity, Severity::Warning))
        .collect();
    let infos: Vec<_> = report
        .findings
        .iter()
        .filter(|f| matches!(f.severity, Severity::Info))
        .collect();

    if !errors.is_empty() {
        fmt.newline()?;
        fmt.write_bold("Errors:")?;
        fmt.newline()?;
        for f in &errors {
            print_finding(f, fmt)?;
        }
    }

    if !warnings.is_empty() {
        fmt.newline()?;
        fmt.write_bold("Warnings:")?;
        fmt.newline()?;
        for f in &warnings {
            print_finding(f, fmt)?;
        }
    }

    if !infos.is_empty() {
        fmt.newline()?;
        fmt.write_bold("Info:")?;
        fmt.newline()?;
        for f in &infos {
            print_finding(f, fmt)?;
        }
    }

    if report.findings.is_empty() {
        fmt.newline()?;
        fmt.success("No issues found.")?;
    }

    Ok(())
}

fn print_finding(finding: &Finding, fmt: &mut Formatter) -> std::io::Result<()> {
    let prefix = match finding.severity {
        Severity::Error => "ERR",
        Severity::Warning => "WRN",
        Severity::Info => "INF",
    };
    let line_ref = finding
        .line
        .map(|l| format!(" (line {l})"))
        .unwrap_or_default();

    let msg = format!("  [{prefix}] [{:>14}]{line_ref} {}", finding.category, finding.message);
    match finding.severity {
        Severity::Error => fmt.error(&msg)?,
        Severity::Warning => fmt.warning(&msg)?,
        Severity::Info => fmt.info(&msg)?,
    }
    if let Some(suggestion) = &finding.suggestion {
        fmt.write_muted(&format!("                  -> {suggestion}"))?;
        fmt.newline()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_tokens() {
        // ~3.5 chars per token
        let text = "Hello world, this is a test.";
        let tokens = estimate_tokens(text);
        assert!(tokens > 0);
        assert!(tokens < 20);
    }

    #[test]
    fn test_extract_sections() {
        let content = "# Title\nsome content\n## Section A\nmore content\nand more\n## Section B\nstuff";
        let lines: Vec<&str> = content.lines().collect();
        let sections = extract_sections(&lines);
        assert_eq!(sections.len(), 3);
        assert_eq!(sections[0].name, "Title");
        assert_eq!(sections[1].name, "Section A");
        assert_eq!(sections[2].name, "Section B");
    }

    #[test]
    fn test_extract_at_imports() {
        let content = "See @README.md for details\nAlso @docs/arch.md\nNo import here\n@notafile";
        let lines: Vec<&str> = content.lines().collect();
        let imports = extract_at_imports(&lines);
        assert_eq!(imports.len(), 2);
        assert_eq!(imports[0].path, "@README.md");
        assert_eq!(imports[1].path, "@docs/arch.md");
    }

    #[test]
    fn test_count_table_lines() {
        let content = "| A | B |\n|---|---|\n| 1 | 2 |\nNot a table\n| 3 | 4 |";
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(count_table_lines(&lines), 4);
    }

    #[test]
    fn test_score_perfect() {
        let score = calculate_score(50, 800, &[]);
        assert_eq!(score, 100);
    }

    #[test]
    fn test_score_too_big() {
        let score = calculate_score(300, 5000, &[]);
        assert!(score <= 50);
    }

    #[test]
    fn test_obvious_detection() {
        let content = "# Rules\n- Write clean code\n- Use meaningful variable names\n";
        let report = analyze(content, Path::new("test.md"));
        let obvious = report
            .findings
            .iter()
            .filter(|f| f.category == "obvious")
            .count();
        assert_eq!(obvious, 2);
    }

    #[test]
    fn test_prohibition_without_alternative() {
        let content = "# Rules\n- Don't use foo\n- Don't use bar; prefer baz instead\n";
        let report = analyze(content, Path::new("test.md"));
        let no_alt = report
            .findings
            .iter()
            .filter(|f| f.category == "no-alternative")
            .count();
        assert_eq!(no_alt, 1); // Only "Don't use foo" lacks alternative
    }
}
