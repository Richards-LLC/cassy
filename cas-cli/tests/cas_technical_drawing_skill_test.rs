//! Distribution and behaviour contracts for the built-in cas-technical-drawing skill.
//!
//! The skill ships a Node renderer (`scripts/draft.mjs`). Beyond the mirror and
//! marker checks, this file materializes the embedded script and example model
//! and proves the two halves of its promise: a clean model renders every sheet
//! kind and passes `check`, and planted projection, proportion, collision,
//! dimension-sum and cut-size defects are each reported as a FAIL.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

#[path = "support/builtin_catalog.rs"]
mod builtin_catalog;

use builtin_catalog::Flavor;

const SKILL: &str = "cas-technical-drawing";

fn skill_files() -> [&'static str; 6] {
    [
        "SKILL.md",
        "references/model-schema.md",
        "references/drafting-conventions.md",
        "references/likeness-critique.md",
        "scripts/draft.mjs",
        "examples/shelf-box.json",
    ]
}

#[test]
fn skill_is_registered_byte_identically_in_every_harness_mirror() {
    for file in skill_files() {
        let relative = format!("skills/{SKILL}/{file}");
        let claude = builtin_catalog::find(Flavor::Claude, &relative);
        for flavor in [Flavor::Codex, Flavor::Grok] {
            let twin = builtin_catalog::find(flavor, &relative);
            assert_eq!(twin, claude, "{relative} differs in the {flavor:?} mirror");
        }
        assert!(
            !claude.contains("mcp__cas__"),
            "{relative} must stay harness-neutral (no CAS MCP prefixes)"
        );
    }
}

#[test]
fn skill_body_is_managed_and_covers_the_workflow() {
    let body = builtin_catalog::find(Flavor::Claude, &format!("skills/{SKILL}/SKILL.md"));
    assert!(body.starts_with("---\n"), "SKILL.md lacks frontmatter");
    assert!(body.contains(&format!("name: {SKILL}")));
    assert!(body.contains("managed_by: cas"));
    assert!(body.contains("description: Use when"));
    for marker in [
        "concept brief",
        "draft.mjs check",
        "draft.mjs render",
        "orthographic",
        "isometric",
        "section",
        "exploded",
        "joint teaching sheet",
        "part cards",
        "likeness critique",
        "--plain",
        "print contract",
        "Never draw geometry by hand",
        "model-schema.md",
        "drafting-conventions.md",
        "likeness-critique.md",
        "--print-width-mm",
    ] {
        assert!(
            body.to_ascii_lowercase().contains(&marker.to_ascii_lowercase()),
            "SKILL.md missing workflow marker {marker:?}"
        );
    }
    let conventions = builtin_catalog::find(
        Flavor::Claude,
        &format!("skills/{SKILL}/references/drafting-conventions.md"),
    );
    for marker in ["0.7", "0.35", "0.25", "Third-angle", "30°", "150°", "chain", "`projection`", "`axis-scale`", "`proportion`", "`collision`", "`dimensions`", "`text-size`", "`cut-list`"] {
        assert!(conventions.contains(marker), "drafting-conventions.md missing {marker:?}");
    }
    let critique = builtin_catalog::find(
        Flavor::Claude,
        &format!("skills/{SKILL}/references/likeness-critique.md"),
    );
    for marker in ["Cold look", "Silhouette", "Proportion", "Part identification", "Floor to ship", "24 / 30", "200 px"] {
        assert!(critique.contains(marker), "likeness-critique.md missing {marker:?}");
    }
}

fn node() -> Option<PathBuf> {
    let output = Command::new("node").arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(PathBuf::from("node"))
}

struct Workspace {
    _dir: TempDir,
    script: PathBuf,
    model: PathBuf,
    out: PathBuf,
}

fn materialize() -> Workspace {
    let dir = TempDir::new().expect("temp dir");
    let script = dir.path().join("draft.mjs");
    fs::write(
        &script,
        builtin_catalog::find(Flavor::Claude, &format!("skills/{SKILL}/scripts/draft.mjs")),
    )
    .expect("write draft.mjs");
    let model = dir.path().join("shelf-box.json");
    fs::write(
        &model,
        builtin_catalog::find(Flavor::Claude, &format!("skills/{SKILL}/examples/shelf-box.json")),
    )
    .expect("write example model");
    let out = dir.path().join("out");
    Workspace { _dir: dir, script, model, out }
}

fn run(node: &Path, script: &Path, args: &[&str]) -> (bool, String) {
    let output = Command::new(node)
        .arg(script)
        .args(args)
        .output()
        .expect("run draft.mjs");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (output.status.success(), text)
}

fn fail_lines<'a>(report: &'a str, check: &str) -> Vec<&'a str> {
    report
        .lines()
        .filter(|line| line.starts_with("FAIL") && line.contains(check))
        .collect()
}

#[test]
fn clean_model_renders_every_sheet_kind_and_passes_every_check() {
    let Some(node) = node() else {
        eprintln!("node not installed; skipping renderer behaviour test");
        return;
    };
    let ws = materialize();
    let out = ws.out.to_string_lossy().to_string();
    let (ok, text) = run(&node, &ws.script, &["render", ws.model.to_str().unwrap(), "--out", &out]);
    assert!(ok, "render failed:\n{text}");
    for kind in ["ortho", "iso", "section-a", "exploded", "joint-1", "joint-2", "parts-1", "parts-list"] {
        let file = ws.out.join(format!("shelf-box-{kind}.svg"));
        assert!(file.exists(), "missing sheet {kind}");
        let svg = fs::read_to_string(&file).unwrap();
        assert!(svg.contains("data-draft=\"1\""), "{kind}: not a draft.mjs sheet");
        assert!(svg.contains("class=\"title-block\""), "{kind}: no title block");
    }
    let ortho = fs::read_to_string(ws.out.join("shelf-box-ortho.svg")).unwrap();
    for marker in ["data-view=\"front\"", "data-view=\"top\"", "data-view=\"right\"", "class=\"hid\"", "class=\"dim\"", "class=\"balloon\"", "class=\"scale-bar\"", "cutting-plane"] {
        assert!(ortho.contains(marker), "ortho sheet missing {marker}");
    }
    let section = fs::read_to_string(ws.out.join("shelf-box-section-a.svg")).unwrap();
    assert!(section.contains("class=\"cut-face\""), "section sheet has no hatched cut faces");
    let joint = fs::read_to_string(ws.out.join("shelf-box-joint-1.svg")).unwrap();
    assert!(joint.contains("SEPARATED") && joint.contains("ASSEMBLED") && joint.contains("SECTION"));

    let (ok, report) = run(&node, &ws.script, &["check", ws.model.to_str().unwrap()]);
    assert!(ok, "check reported failures on the clean model:\n{report}");
    assert!(report.contains("ALL CHECKS PASS"), "unexpected summary:\n{report}");
    for check in ["projection", "axis-scale", "proportion", "collision", "dimensions", "text-size", "cut-size", "joints", "interference"] {
        assert!(report.lines().any(|l| l.starts_with("PASS") && l.contains(check)), "no PASS line for {check}:\n{report}");
    }
}

#[test]
fn planted_svg_defects_are_flagged_by_the_checker() {
    let Some(node) = node() else {
        return;
    };
    let ws = materialize();
    let out = ws.out.to_string_lossy().to_string();
    let (ok, text) = run(&node, &ws.script, &["render", ws.model.to_str().unwrap(), "--out", &out, "--only", "ortho,iso"]);
    assert!(ok, "{text}");
    let iso = fs::read_to_string(ws.out.join("shelf-box-iso.svg")).unwrap();
    let ortho = fs::read_to_string(ws.out.join("shelf-box-ortho.svg")).unwrap();

    // 1. projection: skew one x-axis edge of the isometric by 2 mm.
    let (head, tail) = iso.split_at(iso.find("class=\"obj\"").expect("object edge"));
    let line_start = head.rfind("<line").unwrap();
    let line_end = line_start + iso[line_start..].find("/>").unwrap() + 2;
    let line = &iso[line_start..line_end];
    assert!(line.contains("data-axis="), "edges must carry data-axis for the audit: {line}");
    let y2 = line.split("y2=\"").nth(1).unwrap().split('"').next().unwrap();
    let skewed = line.replace(&format!("y2=\"{y2}\""), &format!("y2=\"{}\"", y2.parse::<f64>().unwrap() + 2.0));
    let planted = format!("{}{}{}", &iso[..line_start], skewed, &iso[line_end..]);
    let _ = tail;
    let file = ws.out.join("planted-projection.svg");
    fs::write(&file, planted).unwrap();
    let (ok, report) = run(&node, &ws.script, &["check", file.to_str().unwrap()]);
    assert!(!ok && !fail_lines(&report, "projection").is_empty(), "skewed isometric edge not flagged:\n{report}");

    // 2. proportion: stretch every line of one part in the front view by 10%.
    let front_start = ortho.find("data-view=\"front\"").unwrap();
    let front_end = front_start + ortho[front_start..].find("</g>").unwrap();
    let mut view = ortho[front_start..front_end].to_string();
    let part = view.split("data-part=\"").nth(1).unwrap().split('"').next().unwrap().to_string();
    let mut stretched = String::new();
    for piece in view.split("<line") {
        if piece.contains(&format!("data-part=\"{part}\"")) && piece.contains("class=\"obj\"") {
            let x2 = piece.split("x2=\"").nth(1).unwrap().split('"').next().unwrap().to_string();
            let x1 = piece.split("x1=\"").nth(1).unwrap().split('"').next().unwrap().to_string();
            let nx2 = x1.parse::<f64>().unwrap() + (x2.parse::<f64>().unwrap() - x1.parse::<f64>().unwrap()) * 1.1 + 3.0;
            stretched.push_str("<line");
            stretched.push_str(&piece.replacen(&format!("x2=\"{x2}\""), &format!("x2=\"{nx2}\""), 1));
        } else {
            if !stretched.is_empty() || piece.starts_with(' ') {
                stretched.push_str("<line");
            }
            stretched.push_str(piece);
        }
    }
    view = stretched.trim_start_matches("<line").to_string();
    let planted = format!("{}{}{}", &ortho[..front_start], view, &ortho[front_end..]);
    let file = ws.out.join("planted-proportion.svg");
    fs::write(&file, planted).unwrap();
    let (ok, report) = run(&node, &ws.script, &["check", file.to_str().unwrap()]);
    assert!(!ok && !fail_lines(&report, "proportion").is_empty(), "stretched part not flagged:\n{report}");

    // 3. collision: drop a numeric label onto the middle of an object edge.
    let obj = ortho.split("class=\"obj\"").next().unwrap();
    let line_start = obj.rfind("<line").unwrap();
    let line = &ortho[line_start..line_start + ortho[line_start..].find("/>").unwrap()];
    let get = |k: &str| line.split(&format!("{k}=\"")).nth(1).unwrap().split('"').next().unwrap().parse::<f64>().unwrap();
    let (mx, my) = ((get("x1") + get("x2")) / 2.0, (get("y1") + get("y2")) / 2.0);
    let planted = ortho.replace("</svg>", &format!("<text x=\"{mx}\" y=\"{}\" font-size=\"3\" text-anchor=\"middle\">12-3/4</text></svg>", my + 1.0));
    let file = ws.out.join("planted-collision.svg");
    fs::write(&file, planted).unwrap();
    let (ok, report) = run(&node, &ws.script, &["check", file.to_str().unwrap()]);
    assert!(!ok && !fail_lines(&report, "collision").is_empty(), "label on an edge not flagged:\n{report}");

    // 4. dimension sum: change the label of the first chained dimension so the chain no longer sums.
    let dim_start = ortho.find("class=\"dim\"").unwrap();
    let text_start = dim_start + ortho[dim_start..].find("<text").unwrap();
    let text_end = text_start + ortho[text_start..].find("</text>").unwrap();
    let text = &ortho[text_start..text_end];
    let label = text.rsplit('>').next().unwrap();
    let planted = format!("{}{}{}", &ortho[..text_start], text.replace(&format!(">{label}"), ">99"), &ortho[text_end..]);
    let file = ws.out.join("planted-dimension.svg");
    fs::write(&file, planted).unwrap();
    let (ok, report) = run(&node, &ws.script, &["check", file.to_str().unwrap()]);
    assert!(!ok && !fail_lines(&report, "dimensions").is_empty(), "wrong dimension label not flagged:\n{report}");
}

#[test]
fn planted_model_defects_are_flagged_by_the_checker() {
    let Some(node) = node() else {
        return;
    };
    let ws = materialize();
    let model = fs::read_to_string(&ws.model).unwrap();

    // cut size disagrees with the placed extents
    let wrong_size = model.replacen("\"size\": [\"12\", \"6\", \"3/4\"]", "\"size\": [\"11\", \"6\", \"3/4\"]", 1);
    assert_ne!(wrong_size, model);
    let file = ws.out.parent().unwrap().join("wrong-size.json");
    fs::write(&file, wrong_size).unwrap();
    let (ok, report) = run(&node, &ws.script, &["check", file.to_str().unwrap(), "--only", "none"]);
    assert!(!ok && !fail_lines(&report, "cut-size").is_empty(), "cut-size mismatch not flagged:\n{report}");

    // stated joint depth disagrees with where the male sits
    let wrong_depth = model.replacen("\"depth\": \"3/8\"", "\"depth\": \"1/4\"", 1);
    let file = ws.out.parent().unwrap().join("wrong-depth.json");
    fs::write(&file, wrong_depth).unwrap();
    let (ok, report) = run(&node, &ws.script, &["check", file.to_str().unwrap(), "--only", "none"]);
    assert!(!ok && !fail_lines(&report, "joints").is_empty(), "joint depth mismatch not flagged:\n{report}");

    // a part moved into another with no joint declared
    let interfering = model.replacen("\"at\": [\"3/8\", 0, \"5\"]", "\"at\": [\"3/8\", 0, \"11\"]", 1);
    assert_ne!(interfering, model);
    let file = ws.out.parent().unwrap().join("interfering.json");
    fs::write(&file, interfering).unwrap();
    let (ok, report) = run(&node, &ws.script, &["check", file.to_str().unwrap(), "--only", "none"]);
    assert!(!ok, "interference not flagged:\n{report}");
    assert!(!fail_lines(&report, "interference").is_empty() || !fail_lines(&report, "joints").is_empty(), "{report}");
}
