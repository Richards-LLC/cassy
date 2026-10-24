//! `cas violet post|thread|read`: Slack through the Violet hub from any
//! repository, with local files given by path (cassy#1157).
//!
//! An agent that shares a file names its path; this command reads, hashes and
//! encodes the bytes itself, so they never enter a tool call or the model's
//! context. Text files travel as `content_encoding: "text"`, binary files as
//! base64. Up to 1 MiB in total goes inline in one call; anything larger uses
//! the hub's `file_external` route (begin, direct upload from disk, complete).
//!
//! The hub is reached with the same `[servers.violet]` registration, bearer,
//! Vercel bypass and allowlist policy the MCP proxy uses ([`hub::ProxyHub`]).
//! Output follows `cas-cli-craft`: a verdict line, the facts, one `->` hint;
//! `--json` prints the hub's receipt (or error receipt) as one document.
//! Hub error codes pass through unchanged and every `ok: false` exits
//! non-zero.

#[cfg(feature = "mcp-proxy")]
pub(crate) mod credentials;
pub mod channels;
pub mod draft_check;
pub mod files;
pub mod hub;

use std::io::Write;
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use serde_json::{Value, json};

use files::{LocalFile, MAX_FILES_PER_MESSAGE, Route, choose_route, inspect_all};
use hub::{Hub, VioletError, call_ok};

/// Replies one `kind: "thread"` call may carry.
pub const MAX_THREAD_REPLIES: usize = 20;

#[derive(Args, Debug, Clone)]
pub struct VioletArgs {
    #[command(subcommand)]
    pub command: VioletCommand,
}

#[derive(Subcommand, Debug, Clone)]
pub enum VioletCommand {
    /// List this project's configured, Violet and Cloud channel bindings
    Channels(channels::ChannelsArgs),
    /// Post a message, or local files by path, to a channel
    Post(PostArgs),
    /// Post a top-level message and its ordered replies as one thread
    Thread(ThreadArgs),
    /// Read a channel since a time, one thread, or one message
    Read(ReadArgs),
}

#[derive(Args, Debug, Clone)]
pub struct PostArgs {
    /// Channel name, for example cas-internal (one leading # is accepted)
    #[arg(long)]
    pub channel: String,
    /// Check the epic's post-release receipt before announcing this release
    #[arg(long)]
    pub release_notes: bool,
    /// Epic whose post-release check authorizes this announcement
    #[arg(long)]
    pub epic: Option<String>,
    /// Message text; with --file it is the files' shared comment
    #[arg(long)]
    pub text: Option<String>,
    /// Post as a reply to this message_id
    #[arg(long = "reply-to", value_name = "MESSAGE_ID")]
    pub reply_to: Option<String>,
    /// Local file to attach, read from disk (repeatable, up to 10)
    #[arg(long = "file", value_name = "PATH")]
    pub files: Vec<PathBuf>,
    /// Reuse a stable request ID for a managed reply retry
    #[arg(long = "request-id", value_name = "UUID", requires = "reply_to")]
    pub request_id: Option<String>,
}

#[derive(Args, Debug, Clone)]
pub struct ThreadArgs {
    /// Channel name, for example cas-internal (one leading # is accepted)
    #[arg(long)]
    pub channel: String,
    /// Check the epic's post-release receipt before announcing this release
    #[arg(long)]
    pub release_notes: bool,
    /// Epic whose post-release check authorizes this announcement
    #[arg(long)]
    pub epic: Option<String>,
    /// Top-level message text
    #[arg(long)]
    pub text: String,
    /// Reply text, posted in the order given (repeatable, up to 20)
    #[arg(long = "reply", value_name = "TEXT")]
    pub replies: Vec<String>,
    /// Attach a local file to the preceding --reply (repeatable)
    #[arg(long = "reply-file", value_name = "PATH")]
    pub reply_files: Vec<PathBuf>,
    /// Stable key for this thread; resending the same thread with it never reposts
    #[arg(long = "idempotency-key", value_name = "KEY")]
    pub idempotency_key: String,
}

#[derive(Args, Debug, Clone)]
pub struct ReadArgs {
    /// Channel name, for example cas-internal (one leading # is accepted)
    #[arg(long)]
    pub channel: String,
    /// RFC 3339 time; messages at or after it (required for a channel read)
    #[arg(long, value_name = "RFC3339")]
    pub since: Option<String>,
    /// Read one thread: its root message_id
    #[arg(long, value_name = "MESSAGE_ID")]
    pub thread: Option<String>,
    /// Read exactly one message or reply
    #[arg(long, value_name = "MESSAGE_ID")]
    pub message: Option<String>,
    /// Continue a partial read with the cursor it returned
    #[arg(long)]
    pub cursor: Option<String>,
    /// Maximum messages, 1-500
    #[arg(long = "max-messages", value_name = "N")]
    pub max_messages: Option<u32>,
}

/// Everything a run needs from outside: the output mode, the raw argument
/// list (only the order of `--reply`/`--reply-file` is read from it), the
/// publication gate, and a lazy hub connection made after local validation.
pub struct RunContext<'a> {
    pub cas_root: Option<&'a Path>,
    pub json: bool,
    pub raw_args: &'a [String],
    pub gate: &'a dyn Fn(&Value) -> Option<String>,
    pub connect: &'a mut dyn FnMut() -> Result<Box<dyn Hub>, VioletError>,
    /// cas-8239 (GH #1300): the caller's current reply-intent fields
    /// (`coordination_operation_id`, `expected_generation`) for a reply to
    /// this message_id, when it holds the thread's claim.
    pub reply_claim: &'a dyn Fn(&str) -> Option<Value>,
}

/// CLI entry point.
pub fn execute(
    args: &VioletArgs,
    cli: &crate::cli::Cli,
    cas_root: Option<&Path>,
) -> anyhow::Result<()> {
    let raw_args: Vec<String> = std::env::args_os()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let gate = |post: &Value| {
        if post.get("kind").and_then(Value::as_str) == Some("message") {
            return crate::hooks::handlers::handlers_events::violet_post_release_denial(post,cas_root);
        }
        crate::hooks::handlers::handlers_events::violet_publication_denial(
            "violet_post",
            Some(post),
            cas_root,
        )
    };
    let mut connect = || -> Result<Box<dyn Hub>, VioletError> { connect_live(cas_root) };
    let reply_claim = |reply_to: &str| cas_root.and_then(|root| current_reply_claim(root, reply_to));
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut context = RunContext {
        cas_root,
        json: cli.json,
        raw_args: &raw_args,
        gate: &gate,
        connect: &mut connect,
        reply_claim: &reply_claim,
    };
    run_violet(&args.command, &mut context, &mut out).map_err(anyhow::Error::new)
}

/// cas-8239: the reply intent this session registered for the thread, read
/// from the project's request journal. No session or no intent is `None`.
fn current_reply_claim(cas_root: &Path, reply_to: &str) -> Option<Value> {
    let session = std::env::var("CAS_SESSION_ID")
        .ok()
        .filter(|id| !id.trim().is_empty())
        .or_else(|| crate::agent_id::read_session_for_mcp(cas_root).ok())?;
    let agent = crate::store::open_agent_store(cas_root).ok()?.get(&session).ok()?;
    crate::coordination::external_work::SqliteRequestJournal::open(cas_root)
        .ok()?
        .current_reply_post(&agent.id, reply_to, chrono::Utc::now())
        .ok()
        .flatten()
}

#[cfg(feature = "mcp-proxy")]
fn connect_live(cas_root: Option<&Path>) -> Result<Box<dyn Hub>, VioletError> {
    Ok(Box::new(hub::ProxyHub::connect(cas_root)?))
}

#[cfg(not(feature = "mcp-proxy"))]
fn connect_live(_cas_root: Option<&Path>) -> Result<Box<dyn Hub>, VioletError> {
    Err(VioletError::local(
        "not_configured",
        "this build has no MCP proxy; rebuild cas with --features mcp-proxy",
    ))
}

/// Run one subcommand, printing its receipt (or, under `--json`, its error
/// receipt) to `out`. The returned error is what makes the exit non-zero.
pub fn run_violet(
    command: &VioletCommand,
    context: &mut RunContext<'_>,
    out: &mut dyn Write,
) -> Result<(), VioletError> {
    let result = match command {
        VioletCommand::Channels(args) => {
            let report = match channels::inspect(context.cas_root, args) {
                Ok(report) => report,
                Err(error) => {
                    if context.json { print_json(out, &error.to_json()); }
                    return Err(error);
                }
            };
            if context.json {
                print_json(out, &serde_json::to_value(&report).expect("serializable channel report"));
            } else {
                let _ = write!(out, "{}", channels::render(&report));
            }
            // Already rendered one complete report; don't wrap it in a post error.
            return if report.ok { Ok(()) } else {
                Err(VioletError::local("bindings_attention", "Slack binding inventory has findings"))
            };
        }
        VioletCommand::Post(args) => run_post(args, context, out),
        VioletCommand::Thread(args) => run_thread(args, context, out),
        VioletCommand::Read(args) => run_read(args, context, out),
    };
    if let Err(error) = &result {
        if context.json {
            print_json(out, &error.to_json());
        } else if let Some(envelope) = &error.envelope {
            // A stopped thread keeps the receipts of what did land.
            render_partial_thread(out, envelope);
        }
    }
    result
}

fn print_json(out: &mut dyn Write, value: &Value) {
    let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    let _ = writeln!(out, "{text}");
}

fn required(value: &str, flag: &str) -> Result<String, VioletError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(VioletError::local(
            "invalid_input",
            format!("{flag} must not be empty"),
        ));
    }
    Ok(value.to_string())
}

fn optional(value: Option<&String>, flag: &str) -> Result<Option<String>, VioletError> {
    value.map(|value| required(value, flag)).transpose()
}

fn gate_check(context: &RunContext<'_>, post: &Value) -> Result<(), VioletError> {
    match (context.gate)(post) {
        Some(denial) => Err(VioletError::local("publication_blocked", denial)),
        None => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// post
// ---------------------------------------------------------------------------

fn run_post(
    args: &PostArgs,
    context: &mut RunContext<'_>,
    out: &mut dyn Write,
) -> Result<(), VioletError> {
    let channel = required(&args.channel, "--channel")?;
    let text = optional(args.text.as_ref(), "--text")?;
    let reply_to = optional(args.reply_to.as_ref(), "--reply-to")?;
    if args.files.len() > MAX_FILES_PER_MESSAGE {
        return Err(VioletError::local(
            "invalid_input",
            format!(
                "{} files given; one message carries at most {MAX_FILES_PER_MESSAGE}",
                args.files.len()
            ),
        ));
    }
    if text.is_none() && args.files.is_empty() {
        return Err(VioletError::local(
            "invalid_input",
            "nothing to post: give --text, --file, or both",
        ));
    }
    let files = inspect_all(&args.files)?;
    if let Some(text) = &text {
        draft_check::check(&[draft_check::DraftMessage {
            label: None,
            text,
            attached: files.iter().map(|file| file.filename.clone()).collect(),
        }])?;
    }
    let route = choose_route(&files);
    // cas-8239 (GH #1300): a reply into a thread this session claimed and
    // registered a reply intent for carries that intent, so the hub posts it
    // as the managed reply. Without one the fields stay off and a managed
    // thread refuses the reply, exactly as before.
    let managed = reply_to.as_deref().and_then(|reply_to| (context.reply_claim)(reply_to));
    // cas-d626: the hub accepts a managed reply only inline, so files past
    // the inline limit fail here instead of as a bare hub refusal.
    if managed.is_some() && route == Route::External {
        let total: u64 = files.iter().map(|file| file.size_bytes).sum();
        return Err(VioletError::local(
            "invalid_input",
            format!(
                "a managed thread reply carries files only inline, at most {} bytes (1 MiB) in total; \
                 these files total {total} bytes. Attach fewer or smaller files",
                files::INLINE_LIMIT
            ),
        ));
    }
    let post = json!({
        "channel": channel,
        "kind": route_kind(route),
        "text": text,
        "release_notes": args.release_notes,
        "epic_id": args.epic,
    });
    if route != Route::Message
        || crate::hooks::handlers::handlers_events::violet_release_notes_post(&post)
    {
        gate_check(context, &post)?;
    }
    let request_id = optional(args.request_id.as_ref(), "--request-id")?;
    let claim = managed.map(|block| {
        json!({
            "coordination_operation_id": block["coordination_operation_id"],
            "expected_generation": block["expected_generation"],
            "request_id": request_id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        })
    });
    let mut hub = (context.connect)()?;
    let receipt = post_files_claimed(hub.as_mut(), &channel, text, reply_to, claim.as_ref(), &files)?;
    if context.json {
        print_json(out, &receipt);
    } else {
        render_post(out, route, &receipt);
    }
    Ok(())
}

fn route_kind(route: Route) -> &'static str {
    match route {
        Route::Message => "message",
        Route::Inline => "file",
        Route::External => "file_external",
    }
}

/// Post text and files, choosing the inline or external route by the files'
/// aggregate size. Returns the hub's final receipt.
pub fn post_files(
    hub: &mut dyn Hub,
    channel: &str,
    text: Option<String>,
    reply_to: Option<String>,
    files: &[LocalFile],
) -> Result<Value, VioletError> {
    post_files_claimed(hub, channel, text, reply_to, None, files)
}

/// [`post_files`] for a managed reply: `claim` (the reply intent's fields
/// and a request_id) goes on every call of the post.
fn post_files_claimed(
    hub: &mut dyn Hub,
    channel: &str,
    text: Option<String>,
    reply_to: Option<String>,
    claim: Option<&Value>,
    files: &[LocalFile],
) -> Result<Value, VioletError> {
    let route = choose_route(files);
    let mut args = json!({ "channel": channel, "kind": route_kind(route) });
    if let Some(reply_to) = reply_to {
        args["reply_to"] = json!(reply_to);
    }
    if let Some(Value::Object(fields)) = claim {
        for (key, value) in fields {
            args[key] = value.clone();
        }
    }
    match route {
        Route::Message => {
            args["text"] = json!(text.unwrap_or_default());
            call_ok(hub, "violet_post", args)
        }
        Route::Inline => {
            if let Some(text) = text {
                args["initial_comment"] = json!(text);
            }
            let entries = files
                .iter()
                .map(LocalFile::inline_entry)
                .collect::<Result<Vec<_>, _>>()?;
            match <[Value; 1]>::try_from(entries) {
                Ok([entry]) => args["file"] = entry,
                Err(entries) => args["files"] = json!(entries),
            }
            call_ok(hub, "violet_post", args)
        }
        Route::External => {
            if let Some(text) = text {
                args["initial_comment"] = json!(text);
            }
            set_external_files(&mut args, files, None);
            args["step"] = json!("begin");
            let slots = begin_uploads(hub, args.clone(), files)?;
            let file_ids: Vec<String> = slots.iter().map(|(id, _)| id.clone()).collect();
            complete_external(hub, args, files, &slots).map_err(|mut error| {
                error.file_ids = file_ids;
                error
            })
        }
    }
}

/// Put `file_external` metadata on a begin or complete call: top-level for
/// one file, `files[]` for several.
fn set_external_files(args: &mut Value, files: &[LocalFile], file_ids: Option<&[String]>) {
    let entry = |index: usize, file: &LocalFile| {
        let mut metadata = file.external_metadata();
        if let Some(ids) = file_ids {
            metadata["file_id"] = json!(ids[index]);
        }
        metadata
    };
    if let [file] = files {
        if let Value::Object(metadata) = entry(0, file) {
            for (key, value) in metadata {
                args[key] = value;
            }
        }
    } else {
        args["files"] = json!(
            files
                .iter()
                .enumerate()
                .map(|(index, file)| entry(index, file))
                .collect::<Vec<_>>()
        );
    }
}

/// Run `begin`, then stream each file from disk to its upload URL. Returns
/// `(file_id, upload_url)` per file, in order.
fn begin_uploads(
    hub: &mut dyn Hub,
    begin: Value,
    files: &[LocalFile],
) -> Result<Vec<(String, String)>, VioletError> {
    let receipt = call_ok(hub, "violet_post", begin)?;
    let slots = upload_slots(&receipt, files)?;
    for ((_, url), file) in slots.iter().zip(files) {
        hub.upload(url, file).map_err(|mut error| {
            error.file_ids = slots.iter().map(|(id, _)| id.clone()).collect();
            error
        })?;
    }
    Ok(slots)
}

fn complete_external(
    hub: &mut dyn Hub,
    mut args: Value,
    files: &[LocalFile],
    slots: &[(String, String)],
) -> Result<Value, VioletError> {
    let ids: Vec<String> = slots.iter().map(|(id, _)| id.clone()).collect();
    args["step"] = json!("complete");
    set_external_files(&mut args, files, Some(&ids));
    call_ok(hub, "violet_post", args)
}

/// Read the begin receipt's `(file_id, upload_url)` pairs: top-level for one
/// file, `files[]` (in request order) for a batch.
fn upload_slots(
    receipt: &Value,
    files: &[LocalFile],
) -> Result<Vec<(String, String)>, VioletError> {
    let bad = |what: &str| {
        VioletError::local(
            "hub_bad_response",
            format!("the hub's file_external begin receipt {what}; nothing was uploaded"),
        )
    };
    let slot = |entry: &Value| -> Option<(String, String)> {
        let id = entry.get("file_id")?.as_str()?.trim();
        let url = entry.get("upload_url")?.as_str()?.trim();
        (!id.is_empty() && !url.is_empty()).then(|| (id.to_string(), url.to_string()))
    };
    if let Some(entries) = receipt.get("files").and_then(Value::as_array) {
        if entries.len() != files.len() {
            return Err(bad("lists a different number of files"));
        }
        let mut slots = Vec::with_capacity(entries.len());
        for (entry, file) in entries.iter().zip(files) {
            if entry
                .get("filename")
                .and_then(Value::as_str)
                .is_some_and(|name| name != file.filename)
            {
                return Err(bad("lists the files in another order"));
            }
            slots.push(slot(entry).ok_or_else(|| bad("lacks a file_id or upload_url"))?);
        }
        return Ok(slots);
    }
    if files.len() == 1 {
        return Ok(vec![
            slot(receipt).ok_or_else(|| bad("lacks a file_id or upload_url"))?,
        ]);
    }
    Err(bad("has no files[] for a batch"))
}

// ---------------------------------------------------------------------------
// thread
// ---------------------------------------------------------------------------

/// One reply as given on the command line: text, files, or both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplySpec {
    pub text: Option<String>,
    pub files: Vec<PathBuf>,
}

/// Recover the order of `--reply` and `--reply-file` from the raw arguments
/// (clap keeps the two lists apart). Each `--reply-file` attaches to the
/// preceding `--reply`; one given before any `--reply` is a file-only reply.
/// The result is cross-checked against clap's own parse.
pub fn order_replies(
    raw_args: &[String],
    texts: &[String],
    files: &[PathBuf],
) -> Result<Vec<ReplySpec>, VioletError> {
    let mut replies: Vec<ReplySpec> = Vec::new();
    let mut tokens = raw_args.iter();
    while let Some(token) = tokens.next() {
        if token == "--" {
            break;
        }
        let (flag, inline) = match token.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value.to_string())),
            _ => (token.as_str(), None),
        };
        let mut value = || inline.clone().or_else(|| tokens.next().cloned());
        match flag {
            "--reply" => {
                if let Some(text) = value() {
                    replies.push(ReplySpec {
                        text: Some(text),
                        files: Vec::new(),
                    });
                }
            }
            "--reply-file" => {
                if let Some(path) = value() {
                    match replies.last_mut() {
                        Some(reply) => reply.files.push(PathBuf::from(path)),
                        None => replies.push(ReplySpec {
                            text: None,
                            files: vec![PathBuf::from(path)],
                        }),
                    }
                }
            }
            // Skip the values of the other options so that one equal to
            // "--reply" is never read as a flag.
            "--channel" | "--text" | "--idempotency-key" => {
                let _ = value();
            }
            _ => {}
        }
    }
    let seen_texts: Vec<&String> = replies.iter().filter_map(|r| r.text.as_ref()).collect();
    let seen_files: Vec<&PathBuf> = replies.iter().flat_map(|r| r.files.iter()).collect();
    if seen_texts != texts.iter().collect::<Vec<_>>()
        || seen_files != files.iter().collect::<Vec<_>>()
    {
        return Err(VioletError::local(
            "invalid_input",
            "could not tell which --reply each --reply-file belongs to; give each --reply-file \
             right after its --reply",
        ));
    }
    Ok(replies)
}

fn run_thread(
    args: &ThreadArgs,
    context: &mut RunContext<'_>,
    out: &mut dyn Write,
) -> Result<(), VioletError> {
    let channel = required(&args.channel, "--channel")?;
    let text = required(&args.text, "--text")?;
    let key = required(&args.idempotency_key, "--idempotency-key")?;
    if key.chars().count() > 200 {
        return Err(VioletError::local(
            "invalid_input",
            "--idempotency-key must be 1-200 characters",
        ));
    }
    let specs = order_replies(context.raw_args, &args.replies, &args.reply_files)?;
    if specs.is_empty() || specs.len() > MAX_THREAD_REPLIES {
        return Err(VioletError::local(
            "invalid_input",
            format!(
                "a thread needs 1-{MAX_THREAD_REPLIES} replies; {} given",
                specs.len()
            ),
        ));
    }
    let mut replies = Vec::with_capacity(specs.len());
    for (index, spec) in specs.iter().enumerate() {
        let text = optional(spec.text.as_ref(), "--reply")?;
        if spec.files.len() > MAX_FILES_PER_MESSAGE {
            return Err(VioletError::local(
                "invalid_input",
                format!(
                    "reply {} has {} files; one reply carries at most {MAX_FILES_PER_MESSAGE}",
                    index + 1,
                    spec.files.len()
                ),
            ));
        }
        replies.push(ReplyPlan {
            text,
            files: inspect_all(&spec.files)?,
        });
    }
    let mut drafts = vec![draft_check::DraftMessage {
        label: Some("top-level".to_string()),
        text: &text,
        attached: Vec::new(),
    }];
    for (index, reply) in replies.iter().enumerate() {
        if let Some(reply_text) = &reply.text {
            drafts.push(draft_check::DraftMessage {
                label: Some(format!("reply {}", index + 1)),
                text: reply_text,
                attached: reply.files.iter().map(|file| file.filename.clone()).collect(),
            });
        }
    }
    draft_check::check(&drafts)?;
    let shares_files = replies.iter().any(|reply| !reply.files.is_empty());
    let mut post = json!({"channel":channel,"kind":"thread","text":text,"release_notes":args.release_notes,"epic_id":args.epic});
    if shares_files {
        post["replies"] = json!([{"files":[]}]);
    }
    if shares_files || crate::hooks::handlers::handlers_events::violet_release_notes_post(&post) {
        gate_check(context, &post)?;
    }
    let mut hub = (context.connect)()?;
    let receipt = post_thread(hub.as_mut(), &channel, &text, &key, &replies)?;
    if context.json {
        print_json(out, &receipt);
    } else {
        render_thread(out, &receipt);
    }
    Ok(())
}

/// One reply, with its files found and hashed.
#[derive(Debug, Clone)]
pub struct ReplyPlan {
    pub text: Option<String>,
    pub files: Vec<LocalFile>,
}

/// Post a thread as one `kind: "thread"` call. Files go inline when the whole
/// thread's files total at most 1 MiB, otherwise every file is uploaded first
/// (begin and direct upload) and its reply carries the external metadata.
pub fn post_thread(
    hub: &mut dyn Hub,
    channel: &str,
    text: &str,
    idempotency_key: &str,
    replies: &[ReplyPlan],
) -> Result<Value, VioletError> {
    let route = choose_route(replies.iter().flat_map(|reply| reply.files.iter()));
    let mut begun: Vec<String> = Vec::new();
    let mut entries = Vec::with_capacity(replies.len());
    let with_begun = |mut error: VioletError, begun: &[String]| {
        let mut ids = begun.to_vec();
        ids.append(&mut error.file_ids);
        error.file_ids = ids;
        error
    };
    for reply in replies {
        let mut entry = json!({});
        if let Some(text) = &reply.text {
            entry["text"] = json!(text);
        }
        if !reply.files.is_empty() {
            let files: Vec<Value> = match route {
                Route::External => {
                    let mut begin =
                        json!({"channel": channel, "kind": "file_external", "step": "begin"});
                    set_external_files(&mut begin, &reply.files, None);
                    let slots = begin_uploads(hub, begin, &reply.files)
                        .map_err(|error| with_begun(error, &begun))?;
                    begun.extend(slots.iter().map(|(id, _)| id.clone()));
                    reply
                        .files
                        .iter()
                        .zip(&slots)
                        .map(|(file, (id, _))| {
                            let mut metadata = file.external_metadata();
                            metadata["file_id"] = json!(id);
                            metadata
                        })
                        .collect()
                }
                _ => reply
                    .files
                    .iter()
                    .map(LocalFile::inline_entry)
                    .collect::<Result<_, _>>()
                    .map_err(|error| with_begun(error, &begun))?,
            };
            entry["files"] = json!(files);
        }
        entries.push(entry);
    }
    let args = json!({
        "channel": channel,
        "kind": "thread",
        "text": text,
        "idempotency_key": idempotency_key,
        "replies": entries,
    });
    let receipt = call_ok(hub, "violet_post", args).map_err(|error| with_begun(error, &begun))?;
    confirm_reply_files(receipt, channel, replies).map_err(|error| with_begun(error, &begun))
}

/// GH #1251: a reply sent with files is confirmed only when its receipt,
/// `posted[i].files`, lists that many distinct `file_id`s in
/// `state: attached`. Anything less fails the run with a named code. The
/// error keeps the hub's receipt, so the posted message ids and any returned
/// file ids are printed, and it never retries.
fn confirm_reply_files(receipt: Value, channel: &str, replies: &[ReplyPlan]) -> Result<Value, VioletError> {
    let posted = |position: usize| {
        receipt["posted"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|item| item["index"].as_u64() == Some(position as u64))
    };
    for (index, reply) in replies.iter().enumerate().filter(|(_, reply)| !reply.files.is_empty()) {
        let position = index + 1;
        let item = posted(position);
        let files: Vec<&Value> = item
            .and_then(|item| item["files"].as_array())
            .into_iter()
            .flatten()
            .filter(|file| file["file_id"].as_str().is_some_and(|id| !id.is_empty()))
            .collect();
        // Distinct ids: two entries for one attached file prove one file.
        let in_state = |state: &str| {
            files
                .iter()
                .filter(|file| file["state"] == state)
                .filter_map(|file| file["file_id"].as_str())
                .collect::<std::collections::BTreeSet<_>>()
                .len()
        };
        let (sent, attached, uploaded) = (reply.files.len(), in_state("attached"), in_state("uploaded"));
        if attached == sent {
            continue;
        }
        let (code, what) = if uploaded > 0 && attached + uploaded >= sent {
            ("uploaded_not_attached", format!("the hub uploaded {uploaded} of them without attaching them to the reply"))
        } else {
            ("thread_files_unconfirmed", format!("the hub's receipt confirms {attached} attached"))
        };
        let root = posted(0).and_then(|item| item["message_id"].as_str()).unwrap_or("unknown");
        let reply_id = item.and_then(|item| item["message_id"].as_str()).unwrap_or("unknown");
        let plural = if sent == 1 { "" } else { "s" };
        // The receipt's error carries the whole cause and remedy for machines;
        // the terminal line stays short because the hint lines name the read
        // command and the same-key rule (cas-e592 terminal-qa).
        let full = format!(
            "reply {position} was sent {sent} file{plural}, but {what}. The thread is posted (root {root}, reply \
             {reply_id}): read it with `cas violet read --channel {channel} --thread {root}` before posting \
             anything. Rerunning with the same --idempotency-key replays this receipt and never reposts; do not \
             repost under a new key."
        );
        let mut error = VioletError::local(
            code,
            format!("reply {position}: {attached}/{sent} files attached; thread posted"),
        );
        let mut envelope = receipt.clone();
        envelope["ok"] = json!(false);
        envelope["failed_index"] = json!(position);
        envelope["resume_safe"] = json!(false);
        envelope["error"] = json!({"code": code, "message": full, "retryable": false});
        error.envelope = Some(envelope);
        return Err(error);
    }
    Ok(receipt)
}

// ---------------------------------------------------------------------------
// read
// ---------------------------------------------------------------------------

fn run_read(
    args: &ReadArgs,
    context: &mut RunContext<'_>,
    out: &mut dyn Write,
) -> Result<(), VioletError> {
    let request = read_request(args)?;
    let mut hub = (context.connect)()?;
    let mut receipt = call_ok(hub.as_mut(), "violet_read", request)?;
    omit_file_content(&mut receipt);
    if context.json {
        print_json(out, &receipt);
    } else {
        render_read(out, &receipt, args);
    }
    Ok(())
}

/// The `violet_read` arguments. A channel read needs `--since`: without it a
/// busy channel exhausts the hub's page budget.
pub fn read_request(args: &ReadArgs) -> Result<Value, VioletError> {
    let channel = required(&args.channel, "--channel")?;
    let since = optional(args.since.as_ref(), "--since")?;
    let thread = optional(args.thread.as_ref(), "--thread")?;
    let message = optional(args.message.as_ref(), "--message")?;
    let cursor = optional(args.cursor.as_ref(), "--cursor")?;
    if let Some(since) = &since
        && chrono::DateTime::parse_from_rfc3339(since).is_err()
    {
        return Err(VioletError::local(
            "invalid_input",
            format!("--since {since:?} is not an RFC 3339 time, for example 2026-10-09T00:00:00Z"),
        ));
    }
    if since.is_none() && thread.is_none() && message.is_none() && cursor.is_none() {
        return Err(VioletError::local(
            "invalid_input",
            "a channel read needs --since (or --thread, --message, --cursor)",
        ));
    }
    if let Some(max) = args.max_messages
        && !(1..=500).contains(&max)
    {
        return Err(VioletError::local(
            "invalid_input",
            "--max-messages must be 1-500",
        ));
    }
    let mut request = json!({ "channel": channel, "include_channels": false });
    for (key, value) in [
        ("since", since),
        ("thread_id", thread),
        ("message_id", message),
        ("cursor", cursor),
    ] {
        if let Some(value) = value {
            request[key] = json!(value);
        }
    }
    if let Some(max) = args.max_messages {
        request["max_messages"] = json!(max);
    }
    Ok(request)
}

/// Drop downloaded file bytes from a read receipt before printing it, so a
/// read never pours a file into an agent's context. Metadata, sizes and
/// hashes stay.
pub fn omit_file_content(receipt: &mut Value) {
    if let Some(files) = receipt.get_mut("files").and_then(Value::as_array_mut) {
        for file in files {
            if let Some(object) = file.as_object_mut()
                && object.remove("content_base64").is_some()
            {
                object.insert("content_omitted".to_string(), json!(true));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Human output
// ---------------------------------------------------------------------------

fn str_at<'a>(value: &'a Value, pointer: &str) -> Option<&'a str> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
}

fn channel_label(receipt: &Value) -> String {
    let name = str_at(receipt, "/channel/name").map(|name| format!("#{name}"));
    let id = str_at(receipt, "/channel/id");
    match (name, id) {
        (Some(name), Some(id)) => format!("{name} ({id})"),
        (Some(name), None) => name,
        (None, Some(id)) => id.to_string(),
        (None, None) => "the channel".to_string(),
    }
}

fn human_size(bytes: u64) -> String {
    const KB: u64 = 1_000;
    const MB: u64 = 1_000_000;
    match bytes {
        bytes if bytes < KB => format!("{bytes} B"),
        bytes if bytes < MB => format!("{} KB", bytes.div_ceil(KB)),
        bytes => format!("{:.1} MB", bytes as f64 / MB as f64),
    }
}

fn file_line(file: &Value) -> String {
    let id = str_at(file, "/file_id").unwrap_or("?");
    let name = str_at(file, "/name")
        .or_else(|| str_at(file, "/filename"))
        .unwrap_or("?");
    let size = file
        .get("size_bytes")
        .and_then(Value::as_u64)
        .map(human_size)
        .unwrap_or_else(|| "? B".to_string());
    let verified = match file.get("sha256_verified").and_then(Value::as_bool) {
        Some(true) => "sha256 verified",
        Some(false) => "sha256 not verified by the hub",
        None => "sha256 not reported",
    };
    format!("file {id} {name} - {size} - {verified}")
}

fn receipt_files(receipt: &Value) -> Vec<&Value> {
    match receipt.get("files").and_then(Value::as_array) {
        Some(files) => files.iter().collect(),
        None => receipt.get("file").into_iter().collect(),
    }
}

fn part_ids(message: &Value) -> Option<String> {
    let ids = message
        .get("part_ids")?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    (!ids.is_empty()).then(|| ids.join(", "))
}

fn render_post(out: &mut dyn Write, route: Route, receipt: &Value) {
    render_contact_warning(out, receipt);
    let files = receipt_files(receipt);
    let detail = match route {
        Route::Message => "message".to_string(),
        _ => format!(
            "{}, {} file{}",
            route.as_str(),
            files.len(),
            if files.len() == 1 { "" } else { "s" }
        ),
    };
    let _ = writeln!(out, "[OK] posted to {} - {detail}", channel_label(receipt));
    match (
        str_at(receipt, "/message/message_id"),
        str_at(receipt, "/message/permalink"),
    ) {
        (Some(id), Some(link)) => {
            let _ = writeln!(out, "     message {id}  {link}");
        }
        (Some(id), None) => {
            let _ = writeln!(out, "     message {id}");
        }
        _ => {
            let _ = writeln!(out, "     message id not resolved by the hub");
        }
    }
    if let Some(ids) = receipt.get("message").and_then(part_ids) {
        let _ = writeln!(out, "     part_ids: {ids}");
    }
    for file in files {
        let _ = writeln!(out, "     {}", file_line(file));
    }
    if let Some(code) = str_at(receipt, "/warning/code") {
        let _ = writeln!(out, "     warning {code}");
    }
}

fn posted_lines(out: &mut dyn Write, receipt: &Value) {
    for item in receipt
        .get("posted")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let index = item
            .get("index")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let role = if index == 0 { "root " } else { "reply" };
        let id = str_at(item, "/message_id").unwrap_or("?");
        let link = str_at(item, "/permalink").unwrap_or("");
        let _ = writeln!(out, "     {index} {role} {id}  {link}");
        if let Some(ids) = part_ids(item) {
            let _ = writeln!(out, "         part_ids: {ids}");
        }
        for file in item
            .get("files")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let _ = writeln!(out, "         {}", file_line(file));
        }
    }
}

fn render_thread(out: &mut dyn Write, receipt: &Value) {
    render_contact_warning(out, receipt);
    let count = receipt
        .get("posted")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let _ = writeln!(
        out,
        "[OK] posted thread to {} - {count} message{}",
        channel_label(receipt),
        if count == 1 { "" } else { "s" }
    );
    posted_lines(out, receipt);
}

fn render_partial_thread(out: &mut dyn Write, envelope: &Value) {
    render_contact_warning(out, envelope);
    if envelope.get("kind").and_then(Value::as_str) != Some("thread") {
        return;
    }
    let failed = envelope
        .get("failed_index")
        .and_then(Value::as_u64)
        .map_or_else(|| "?".to_string(), |index| index.to_string());
    let resume_safe = envelope.get("resume_safe").and_then(Value::as_bool) == Some(true);
    let unconfirmed = matches!(
        str_at(envelope, "/error/code"),
        Some("thread_files_unconfirmed" | "uploaded_not_attached")
    );
    if unconfirmed {
        let _ = writeln!(out, "[FAILED] reply {failed} files not confirmed attached; the thread was posted");
        posted_lines(out, envelope);
        // The channel id is bounded where a channel name is not, so the read
        // command stays within 80 columns on its own line.
        let channel = str_at(envelope, "/channel/id")
            .or_else(|| str_at(envelope, "/channel/name"))
            .unwrap_or("<channel>");
        let root = str_at(envelope, "/posted/0/message_id").unwrap_or("<root>");
        let _ = writeln!(out, "  -> read the thread before posting again:");
        let _ = writeln!(out, "     cas violet read --channel {channel} --thread {root}");
        let _ = writeln!(out, "  -> rerun only with the same --idempotency-key, never a new key");
        return;
    }
    let _ = writeln!(
        out,
        "[FAILED] thread to {} stopped at index {failed}; resume_safe {resume_safe}",
        channel_label(envelope)
    );
    posted_lines(out, envelope);
    let hint = if resume_safe {
        "retry the identical command with the same --idempotency-key"
    } else {
        "read the thread first: cas violet read --thread <root>"
    };
    let _ = writeln!(out, "  -> {hint}");
}

fn render_contact_warning(out: &mut dyn Write, receipt: &Value) {
    if let Some(warning) = receipt["dispatch_warning"].as_str() {
        let _ = writeln!(out, "[WARN] {warning}");
    }
}

fn render_read(out: &mut dyn Write, receipt: &Value, args: &ReadArgs) {
    let messages: Vec<&Value> = receipt
        .get("messages")
        .and_then(Value::as_array)
        .map(|messages| messages.iter().collect())
        .unwrap_or_default();
    let complete = receipt.get("complete").and_then(Value::as_bool) == Some(true);
    let _ = writeln!(
        out,
        "[OK] {} - {} message{}, {}",
        channel_label(receipt),
        messages.len(),
        if messages.len() == 1 { "" } else { "s" },
        if complete { "complete" } else { "partial" }
    );
    let files: Vec<&Value> = receipt
        .get("files")
        .and_then(Value::as_array)
        .map(|files| files.iter().collect())
        .unwrap_or_default();
    for message in messages {
        let id = str_at(message, "/message_id").unwrap_or("?");
        let author = str_at(message, "/author/name")
            .or_else(|| str_at(message, "/author/id"))
            .unwrap_or("?");
        let when = str_at(message, "/created_at").unwrap_or("");
        let is_reply = str_at(message, "/thread_id").is_some_and(|thread| thread != id);
        let indent = if is_reply { "    " } else { "" };
        let _ = writeln!(
            out,
            "{indent}{id}  {author}  {when}{}",
            if is_reply { "  (reply)" } else { "" }
        );
        for line in message
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .lines()
        {
            let _ = writeln!(out, "{indent}     {line}");
        }
        for file_id in message
            .get("file_ids")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            match files
                .iter()
                .find(|file| file.get("file_id").and_then(Value::as_str) == Some(file_id))
            {
                Some(file) => {
                    let name = str_at(file, "/name").unwrap_or("?");
                    let size = file
                        .get("size_bytes")
                        .and_then(Value::as_u64)
                        .map(human_size)
                        .unwrap_or_else(|| "? B".to_string());
                    let skipped = str_at(file, "/skip_reason")
                        .map(|reason| format!(" - skipped: {reason}"))
                        .unwrap_or_default();
                    let _ = writeln!(out, "{indent}     file {file_id} {name} - {size}{skipped}");
                }
                None => {
                    let _ = writeln!(out, "{indent}     file {file_id}");
                }
            }
        }
    }
    for failed in receipt
        .get("threads_failed")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let thread = str_at(failed, "/thread_id").unwrap_or("?");
        let code = str_at(failed, "/code").unwrap_or("?");
        let _ = writeln!(out, "     thread {thread} not expanded ({code})");
    }
    if let Some(cursor) = str_at(receipt, "/cursor") {
        let _ = writeln!(
            out,
            "  -> more: cas violet read --channel {} --cursor {cursor} (same filters)",
            args.channel.trim()
        );
    }
}

#[cfg(test)]
mod tests;
