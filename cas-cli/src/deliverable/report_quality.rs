//! Report quality (cas-5d07): what a cas-html-reports report must carry
//! before the operator sees it.
//!
//! The close gate runs this over every report a delivery changes: an
//! `.html` under `docs/` beside a same-basename `.md`. Visual QA proves the
//! page renders; this proves it is the report the skill describes. Each
//! finding names exactly what is missing, so the worker can fix it in one
//! pass:
//!
//! - the concept brief (`<basename>.brief.md`) naming its audience, with a
//!   `## Critique` table that meets the cas-ui-craft floor (distinctiveness,
//!   fit and hierarchy at least 4, no dimension 0);
//! - the DESIGN.md brand tokens declared as CSS custom properties, and no
//!   network fetch (web fonts included);
//! - one verdict first: a `data-verdict` element before any `<h2>` or table;
//! - a page-height budget for the audience, estimated from the open content
//!   (collapsed `<details>` count as their summary);
//! - for executive and external readers, no task ids anywhere and no
//!   internal jargon in the verdict or above the first section.

use std::sync::LazyLock;

use regex::Regex;

use super::Finding;

/// Semantic roles every report declares from the project's DESIGN.md
/// (cas-html-reports technical contract §4).
pub const BRAND_TOKENS: &[&str] = &["bg", "surface", "surface-hero", "ink", "line", "verdict", "evidence"];

/// The cas-ui-craft floor for a public surface.
pub const FLOOR_DIMENSIONS: &[&str] = &["distinctiveness", "fit", "hierarchy"];
/// The rubric's five dimensions; every one must be scored, 1 to 5.
const RUBRIC_DIMENSIONS: &[&str] = &["distinctiveness", "fit", "hierarchy", "craft", "accessibility"];
pub const FLOOR: u8 = 4;

/// Estimated page height at 1280 px, by audience: five screens for a
/// business reader, ten for a practitioner.
pub const BUSINESS_HEIGHT_BUDGET: u32 = 4_000;
pub const PRACTITIONER_HEIGHT_BUDGET: u32 = 8_000;

const VERDICT_MAX_WORDS: usize = 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportAudience {
    Executive,
    Practitioner,
    External,
}

impl ReportAudience {
    fn business(self) -> bool {
        !matches!(self, Self::Practitioner)
    }

    fn height_budget(self) -> u32 {
        if self.business() { BUSINESS_HEIGHT_BUDGET } else { PRACTITIONER_HEIGHT_BUDGET }
    }

    fn with_article(self) -> &'static str {
        match self {
            Self::Executive => "an executive",
            Self::Practitioner => "a practitioner",
            Self::External => "an external",
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Executive => "executive",
            Self::Practitioner => "practitioner",
            Self::External => "external",
        }
    }
}

macro_rules! re {
    ($name:ident, $pattern:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pattern).expect("static regex"));
    };
}

re!(AUDIENCE, r"(?im)\baudience\s*[:*]+\s*\**\s*(executive|practitioner|external)\b");
re!(CRITIQUE_HEADING, r"(?m)^##\s+Critique\b");
re!(NEXT_HEADING, r"(?m)^##\s");
re!(INVISIBLE, r"(?is)<!--.*?-->|<script\b.*?</script>|<style\b.*?</style>|<template\b.*?</template>|<head\b.*?</head>");
re!(DETAILS, r"(?is)<details\b[^>]*>.*?</details>");
re!(SUMMARY, r"(?is)<summary\b.*?</summary>");
re!(TAG, r"(?s)<[^>]*>");
re!(SPACE, r"\s+");
re!(ROW, r"(?i)<tr\b");
re!(FIGURE_BLOCK, r"(?is)<figure\b.*?</figure>");
re!(FIGURE, r"(?i)<figure\b");
re!(MEDIA, r"(?i)<(?:img|svg|canvas|video)\b");
re!(TABLE_BLOCK, r"(?is)<table\b.*?</table>");
re!(SVG_BLOCK, r"(?is)<svg\b.*?</svg>");
re!(HEADING, r"(?i)<h[1-3]\b");
re!(BLOCK, r"(?i)<(?:p|li|h[1-6]|dt|dd|blockquote|pre|div)\b");
re!(NETWORK, r#"(?i)(?:<(?:link|script|img|iframe|source|video|audio)\b[^>]*\b(?:href|src)\s*=\s*["']?\s*(?:https?:)?//)|@import\b|url\(\s*["']?\s*(?:https?:)?//|fonts\.googleapis\.com|fonts\.gstatic\.com"#);
re!(VERDICT_OPEN, r#"(?i)<([a-z][a-z0-9]*)\b[^>]*\bdata-verdict\b[^>]*>"#);
re!(SECTION_START, r"(?i)<(?:h2|table)\b");
re!(FIRST_H2, r"(?i)<h2\b");
re!(TASK_ID, r"\bcas-[0-9a-f]{4,5}\b");
re!(JARGON, r"(?i)\b(?:worker|supervisor|factory|epic|lane|nextest|cargo|merge queue|close gate|pull request|PR #\d+|commit [0-9a-f]{7,}|[\w./-]+\.(?:rs|ts|tsx|py|toml|ya?ml|json))\b");

fn custom_property(name: &str) -> Regex {
    Regex::new(&format!(r"--{}\s*:", regex::escape(name))).expect("token regex")
}

fn visible_text(html: &str) -> String {
    let text = TAG.replace_all(html, " ");
    let text = htmlescape_lite(&text);
    SPACE.replace_all(&text, " ").trim().to_owned()
}

/// Enough entity decoding to count and search visible text.
fn htmlescape_lite(text: &str) -> String {
    text.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&rarr;", "→")
        .replace("&middot;", "·")
}

/// The page height at 1280 px, estimated from its open content. Calibrated
/// on the shipped exemplars (measured 2190–2777 px, estimated 2613–2781 px)
/// and on this repository's reports (within about a third either way).
pub fn estimated_height(html: &str) -> u32 {
    let body = INVISIBLE.replace_all(html, "");
    let body = DETAILS.replace_all(&body, |captures: &regex::Captures<'_>| {
        let summary = SUMMARY.find(&captures[0]).map(|found| found.as_str()).unwrap_or("");
        format!("{summary}<p>")
    });
    let rows = ROW.find_iter(&body).count();
    let figures = FIGURE.find_iter(&body).count();
    let unboxed = FIGURE_BLOCK.replace_all(&body, "");
    let media = MEDIA.find_iter(&unboxed).count();
    let table_chars: usize = TABLE_BLOCK.find_iter(&unboxed).map(|table| visible_text(table.as_str()).chars().count()).sum();
    let prose = TABLE_BLOCK.replace_all(&unboxed, "");
    let prose = SVG_BLOCK.replace_all(&prose, "");
    let headings = HEADING.find_iter(&prose).count();
    let blocks = BLOCK.find_iter(&prose).count();
    let chars = visible_text(&prose).chars().count();
    let height = 200.0
        + headings as f64 * 48.0
        + (figures + media) as f64 * 360.0
        + rows as f64 * 20.0
        + table_chars as f64 / 60.0 * 24.0
        + chars as f64 / 80.0 * 27.0
        + blocks as f64 * 8.0;
    height as u32
}

/// The scores of a brief's `## Critique` table, by lowercased first word of
/// the dimension ("fit to argument" is "fit").
fn critique_scores(brief: &str) -> Option<Vec<(String, u8)>> {
    let start = CRITIQUE_HEADING.find(brief)?.end();
    let section = &brief[start..];
    let section = NEXT_HEADING.find(section).map_or(section, |next| &section[..next.start()]);
    let scores: Vec<(String, u8)> = section
        .lines()
        .filter_map(|line| {
            let cells: Vec<&str> = line.trim().strip_prefix('|')?.split('|').map(str::trim).collect();
            let dimension = cells.first()?.trim_matches('*').split_whitespace().next()?.to_lowercase();
            let score = cells.get(1)?.trim_matches('*').parse::<u8>().ok()?;
            Some((dimension, score))
        })
        .collect();
    (!scores.is_empty()).then_some(scores)
}

fn finding(code: &str, message: String) -> Finding {
    Finding { code: code.into(), message }
}

/// Every reason `html` (with its concept brief, when one exists) is not yet
/// a report the operator should see. Empty means ready.
pub fn check(html: &str, brief: Option<&str>) -> Vec<Finding> {
    let mut findings = Vec::new();
    let audience = brief.and_then(|brief| AUDIENCE.captures(brief)).map(|found| match found[1].to_lowercase().as_str() {
        "practitioner" => ReportAudience::Practitioner,
        "external" => ReportAudience::External,
        _ => ReportAudience::Executive,
    });
    match brief {
        None => findings.push(finding("brief-missing", "no concept brief: commit `<basename>.brief.md` beside the report (cas-html-reports step 3), naming `Audience: executive|practitioner|external`, with the `## Critique` table".into())),
        Some(brief) => {
            if audience.is_none() {
                findings.push(finding("audience-missing", "the brief names no audience: add `Audience: executive`, `practitioner` or `external` (report-types.md)".into()));
            }
            match critique_scores(brief) {
                None => findings.push(finding("critique-missing", "the brief has no `## Critique` table: score the render with the cas-ui-craft rubric (critique-rubric.md) and append the table".into())),
                Some(scores) => {
                    let mut short = Vec::new();
                    for dimension in RUBRIC_DIMENSIONS {
                        match scores.iter().find(|(name, _)| name == dimension) {
                            None => short.push(format!("{dimension} not scored")),
                            Some((_, score)) if *score > 5 => short.push(format!("{dimension} {score} (scores run 1 to 5)")),
                            Some((_, score)) if FLOOR_DIMENSIONS.contains(dimension) && *score < FLOOR => short.push(format!("{dimension} {score}")),
                            Some(_) => {}
                        }
                    }
                    short.extend(scores.iter().filter(|(_, score)| *score == 0).map(|(name, _)| format!("{name} 0 (mechanical defect)")));
                    if !short.is_empty() {
                        findings.push(finding("critique-floor", format!("the critique is below the floor (distinctiveness, fit and hierarchy at least {FLOOR}, no dimension 0): {}; rework the hero or the distinctive move and re-score", short.join(", "))));
                    }
                }
            }
        }
    }
    let missing: Vec<String> = BRAND_TOKENS.iter().filter(|token| !custom_property(token).is_match(html)).map(|token| format!("--{token}")).collect();
    if !missing.is_empty() {
        findings.push(finding("brand-tokens", format!("the DESIGN.md brand tokens are not declared: {}; resolve them with `cas deliverable brand` and declare them in `:root` (technical-contract.md §4)", missing.join(", "))));
    }
    if let Some(fetch) = NETWORK.find(html) {
        let shown: String = fetch.as_str().chars().take(60).collect();
        findings.push(finding("network", format!("the report fetches from the network (`{shown}`); use the system font stack and inline every asset")));
    }
    let body = INVISIBLE.replace_all(html, "");
    match VERDICT_OPEN.captures(&body) {
        None => findings.push(finding("verdict-missing", "no verdict: mark the one or two sentence answer with `data-verdict` (usually the `<h1>`), first on the page".into())),
        Some(open) => {
            let whole = open.get(0).expect("match");
            if SECTION_START.find(&body[..whole.start()]).is_some() {
                findings.push(finding("verdict-not-first", "the verdict comes after a section or table; move the `data-verdict` element above the first `<h2>` and table".into()));
            }
            let close = format!("</{}", &open[1]);
            let rest = &body[whole.end()..];
            let inner = rest.to_lowercase().find(&close.to_lowercase()).map_or(rest, |end| &rest[..end]);
            let words = visible_text(inner).split_whitespace().count();
            if words == 0 || words > VERDICT_MAX_WORDS {
                findings.push(finding("verdict-length", format!("the verdict is {words} words; state it in one or two sentences (at most {VERDICT_MAX_WORDS} words)")));
            }
        }
    }
    let effective = audience.unwrap_or(ReportAudience::Executive);
    let height = estimated_height(html);
    if height > effective.height_budget() {
        findings.push(finding("height-budget", format!("the page runs about {height} px at 1280 px wide, over the {} px budget for {} reader; keep the argument and move case-by-case detail into a collapsed `<details>` or the markdown", effective.height_budget(), effective.with_article())));
    }
    if effective.business() {
        let text = visible_text(&body);
        let ids: std::collections::BTreeSet<&str> = TASK_ID.find_iter(&text).map(|found| found.as_str()).collect();
        if !ids.is_empty() {
            findings.push(finding("internal-ids", format!("a {} reader sees internal task ids: {}; name the work in plain words", effective.as_str(), ids.into_iter().take(5).collect::<Vec<_>>().join(", "))));
        }
        let top = FIRST_H2.find(&body).map_or(&body[..], |h2| &body[..h2.start()]);
        let jargon: std::collections::BTreeSet<String> = JARGON.find_iter(&visible_text(top)).map(|found| found.as_str().to_lowercase()).collect();
        if !jargon.is_empty() {
            findings.push(finding("jargon", format!("the top of a {} report uses internal vocabulary: {}; say what it means for the reader", effective.as_str(), jargon.into_iter().take(5).collect::<Vec<_>>().join(", "))));
        }
    }
    findings
}

/// The `(markdown, brief)` paths of the report a changed `.html` path is, if
/// it is one: under `docs/`, beside a tracked same-basename `.md`.
/// Release, feature, SOW and customer deliverables, and QA harnesses, keep
/// their own contracts.
pub fn report_paths(html_path: &str, tracked: &[String]) -> Option<(String, String)> {
    let stem = html_path.strip_suffix(".html")?;
    if !html_path.starts_with("docs/")
        || ["docs/release-reports/", "docs/deliverables/", "docs/features/", "docs/sow/", "docs/qa/"].iter().any(|prefix| html_path.starts_with(prefix))
    {
        return None;
    }
    let md = format!("{stem}.md");
    tracked.iter().any(|path| *path == md).then(|| (md, format!("{stem}.brief.md")))
}
