//! One PR comment per gate, edited in place: the comment carrying [`MARKER`]
//! is updated; only when none exists is a new one created.
use super::pack::Pack;
use super::printable;
use super::verdict::GateVerdict;
use std::time::Duration;

pub const MARKER: &str = "<!-- cassy:release-gate -->";
/// GitHub refuses comment bodies over 65,536 characters; stay well under.
pub const MAX_CHARS: usize = 65_000;

/// The GitHub comment calls the gate makes. Production is [`GhComments`];
/// tests use a fake.
pub trait PrComments {
    /// `(id, body)` of each comment on the PR.
    fn list(&self, repo: &str, pr: u64) -> Result<Vec<(u64, String)>, String>;
    fn create(&self, repo: &str, pr: u64, body: &str) -> Result<u64, String>;
    fn update(&self, repo: &str, id: u64, body: &str) -> Result<(), String>;
}

/// The pack as a PR comment: gate verdict, failed and owed rows, PR coverage,
/// hand-walk debt, the flag snapshot and the media count. A body over [`MAX_CHARS`] is cut at
/// a line and ends with a pointer to the full pack.
pub fn render_comment(pack: &Pack) -> String {
    let gate = &pack.gate;
    let word = match gate.verdict {
        GateVerdict::Clean => "CLEAN",
        GateVerdict::Failed => "FAILED",
        GateVerdict::Owed => "OWED",
    };
    let mut body = format!(
        "{MARKER}\n### Cassy release gate: {word} for `{}`\n\n{} rows",
        &pack.sha[..pack.sha.len().min(12)],
        pack.rows.len()
    );
    let passing = pack.rows.iter().filter(|r| r.verdict.word() == "PASS").count();
    body.push_str(&format!(", {passing} PASS, {} FAIL, {} owed\n", gate.failed.len(), gate.owed.len()));
    if !gate.failed.is_empty() {
        body.push_str("\n**Failed**\n");
        for id in &gate.failed {
            let cause = pack.rows.iter().find(|r| &r.id == id).map(|r| r.verdict.to_string()).unwrap_or_default();
            body.push_str(&format!("- `{}`: {}\n", printable(id), printable(&cause)));
        }
    }
    if !gate.owed.is_empty() {
        body.push_str("\n**Owed**\n");
        for owed in &gate.owed {
            body.push_str(&format!("- {}: {}\n", printable(&owed.subject), printable(&owed.cause)));
        }
    }
    if !pack.coverage.is_empty() {
        body.push_str("\n**PR coverage**\n");
        for pr in &pack.coverage {
            let by = match (&pr.manual_row, pr.rows.is_empty()) {
                (_, false) => pr.rows.iter().map(|r| format!("`{}`", printable(r))).collect::<Vec<_>>().join(", "),
                (Some(manual), true) => format!("manual row `{}`", printable(manual)),
                (None, true) => "**not exercised**".into(),
            };
            body.push_str(&format!("- {}: {by}\n", pr.subject()));
        }
    }
    if let Some(summary) = pack.hand_walk.as_ref().and_then(|h| h.summary(3)) {
        body.push_str(&format!("\n**Hand-walk debt** (never changes the verdict): {summary}\n"));
    }
    if let Some(flags) = &pack.flags {
        body.push_str(&format!("\n`{}`\n", flags.ledger_line()));
    }
    body.push_str(&format!("\n{} media files kept locally.\n", pack.media.len()));
    if body.chars().count() <= MAX_CHARS {
        return body;
    }
    let pointer = format!(
        "\n… truncated. The full record is `release/{}/pack.json` in the project's artifacts.\n",
        pack.sha
    );
    let keep = MAX_CHARS - pointer.chars().count();
    let cut = body.char_indices().nth(keep).map_or(body.len(), |(index, _)| index);
    let cut = body[..cut].rfind('\n').map_or(cut, |line| line + 1);
    body.truncate(cut);
    body.push_str(&pointer);
    body
}

/// Updates the marked comment on `pr`, or creates it. Returns its id.
pub fn publish_comment(gh: &impl PrComments, repo: &str, pr: u64, pack: &Pack) -> Result<u64, String> {
    let body = render_comment(pack);
    match gh.list(repo, pr)?.into_iter().find(|(_, text)| text.starts_with(MARKER)) {
        Some((id, _)) => gh.update(repo, id, &body).map(|()| id),
        None => gh.create(repo, pr, &body),
    }
}

/// `gh api`, bounded, honouring the `CAS_QA_GH` executable override.
pub struct GhComments;

impl GhComments {
    fn api(&self, args: &[&str]) -> Result<String, String> {
        let program = std::env::var_os(crate::qa_pass::github_gate::GH_BIN_ENV)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "gh".into());
        let mut command = std::process::Command::new(program);
        command.arg("api").args(args);
        let cap = Duration::from_secs(20);
        let output = crate::bounded_process::run_command(&mut command, crate::bounded_process::Deadline::after(cap), cap)
            .map_err(|e| format!("gh api: {e:?}"))?;
        if !output.status.success() {
            return Err(format!("gh api exited {}", output.status));
        }
        String::from_utf8(output.stdout).map_err(|_| "gh api output is not UTF-8".into())
    }
}

impl PrComments for GhComments {
    fn list(&self, repo: &str, pr: u64) -> Result<Vec<(u64, String)>, String> {
        let path = format!("repos/{repo}/issues/{pr}/comments?per_page=100");
        self.api(&["--paginate", &path, "--jq", ".[] | {id, body}"])?
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let value: serde_json::Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
                let id = value["id"].as_u64().ok_or("comment without id")?;
                Ok((id, value["body"].as_str().unwrap_or_default().to_owned()))
            })
            .collect()
    }

    fn create(&self, repo: &str, pr: u64, body: &str) -> Result<u64, String> {
        let path = format!("repos/{repo}/issues/{pr}/comments");
        let field = format!("body={body}");
        self.api(&["-X", "POST", &path, "-f", &field, "--jq", ".id"])?
            .trim()
            .parse()
            .map_err(|_| "gh api returned no comment id".into())
    }

    fn update(&self, repo: &str, id: u64, body: &str) -> Result<(), String> {
        let path = format!("repos/{repo}/issues/comments/{id}");
        let field = format!("body={body}");
        self.api(&["-X", "PATCH", &path, "-f", &field]).map(|_| ())
    }
}
