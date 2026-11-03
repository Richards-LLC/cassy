//! Bounded Violet context newer than the handoff being resumed.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{OnceLock, mpsc};
use std::time::Duration;

use cas_types::Entry;
use chrono::{DateTime, Utc};
use regex::Regex;
use serde_json::{Value, json};

const HANDOFF: &str = "## 🔁 Current Handoff";
const HEADING: &str = "## Messages newer than this handoff";
const COULD_NOT_CHECK: &str = "Couldn't check all messages newer than this handoff (Violet/Cloud unavailable, partial read, or read cap reached).";
const CHECK_BUDGET: Duration = Duration::from_secs(4);
const MAX_READS: usize = 6;
const MAX_POSTS: usize = 10;
const SECTION_BYTES: usize = 1800;

/// Only SessionStart calls this. Cloud memory lookup, proxy setup, reads and
/// shutdown all share one deadline; an unavailable service cannot hold startup.
pub(super) fn inject(context: String, cas_root: &Path) -> String {
    let Some(id) = handoff_id(&context) else {
        return context;
    };
    let shown = context
        .split_once(HANDOFF)
        .map(|(_, tail)| tail.split("\n## ").next().unwrap_or(tail))
        .unwrap_or_default();
    if !shown.contains("_Handoff truncated.") && targets(shown).is_empty() {
        return context;
    }
    let root = cas_root.to_path_buf();
    let base = context.clone();
    bounded(CHECK_BUDGET, move || {
        let store = crate::store::open_store(&root).map_err(|_| ())?;
        let handoff = store.get(&id).map_err(|_| ())?;
        if handoff.created.timestamp() < 0 {
            return Err(());
        }
        check_violet(&base, &handoff, &root)
    })
    .unwrap_or_else(|| insert_section(&context, COULD_NOT_CHECK))
}

fn handoff_id(context: &str) -> Option<String> {
    let mut lines = context
        .lines()
        .skip_while(|line| !line.starts_with(HANDOFF));
    lines.next()?;
    lines
        .take_while(|line| !line.starts_with("## "))
        .find_map(|line| {
            line.strip_prefix("### ")?
                .split_whitespace()
                .next()
                .map(str::to_string)
        })
}

fn bounded<T: Send + 'static>(
    budget: Duration,
    operation: impl FnOnce() -> Result<T, ()> + Send + 'static,
) -> Result<T, ()> {
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("handoff-violet-check".into())
        .spawn(move || {
            let _ = sender.send(operation());
        })
        .map_err(|_| ())?;
    receiver.recv_timeout(budget).map_err(|_| ())?
}

#[cfg(feature = "mcp-proxy")]
fn check_violet(context: &str, handoff: &Entry, root: &Path) -> Result<String, ()> {
    use crate::cli::violet_cmd::hub::{cli_caller, connect_config, decode_tool_result};
    use cmcp_core::{ProxyEngine, config::VIOLET_SERVER};

    if targets(&references(handoff)).is_empty() {
        return Ok(context.into());
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let config = connect_config(Some(root)).map_err(|_| ())?;
    let caller = cli_caller(Some(root));
    let bots = crate::config::Config::load(root)
        .unwrap_or_default()
        .slack
        .unwrap_or_default()
        .violet_bot_user_ids();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| ())?;
    let connected = runtime.block_on(async {
        tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), async {
            let engine = ProxyEngine::from_configs(config.servers.clone())
                .await
                .map_err(|_| ())?;
            crate::mcp::install_proxy_policy(&engine, &config).await;
            Ok::<_, ()>(engine)
        })
        .await
    });
    let result = match connected {
        Ok(Ok(engine)) => {
            let enriched = enrich(context, handoff, &bots, |args| {
                let result = runtime
                    .block_on(async {
                        tokio::time::timeout_at(
                            tokio::time::Instant::from_std(deadline),
                            engine.call_tool(
                                &caller,
                                VIOLET_SERVER,
                                "violet_read",
                                args.as_object().cloned(),
                            ),
                        )
                        .await
                    })
                    .map_err(|_| "timeout")?
                    .map_err(|_| "Violet unavailable")?;
                if result["isError"] == true {
                    return Err("Violet refused the read".into());
                }
                decode_tool_result(&result).map_err(|_| "invalid Violet receipt".to_string())
            });
            let _ = runtime.block_on(async {
                tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), engine.shutdown())
                    .await
            });
            Ok(enriched)
        }
        _ => Err(()),
    };
    runtime.shutdown_timeout(Duration::from_millis(50));
    result
}

#[cfg(not(feature = "mcp-proxy"))]
fn check_violet(context: &str, handoff: &Entry, _: &Path) -> Result<String, ()> {
    if targets(&references(handoff)).is_empty() {
        Ok(context.into())
    } else {
        Err(())
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Target {
    channel: String,
    thread: Option<String>,
}

fn references(handoff: &Entry) -> String {
    format!(
        "{}\n{}",
        handoff.title.as_deref().unwrap_or_default(),
        handoff.content
    )
}

/// A named thread also contributes a channel read, so sibling operator posts
/// are found. Read the thread explicitly too: it may predate Violet's lookback.
fn targets(body: &str) -> Vec<Target> {
    static LINKS: OnceLock<Regex> = OnceLock::new();
    static NAMES: OnceLock<Regex> = OnceLock::new();
    static IDS: OnceLock<Regex> = OnceLock::new();
    let mut targets = Vec::new();
    let mut add = |channel: String, thread: Option<String>| {
        let target = Target { channel, thread };
        if !targets.contains(&target) {
            targets.push(target);
        }
    };
    for link in LINKS
        .get_or_init(|| Regex::new(r"https://[A-Za-z0-9.-]+\.slack\.com/[^\s<>`)]+").unwrap())
        .find_iter(body)
    {
        let Ok(url) = url::Url::parse(link.as_str()) else {
            continue;
        };
        let parts: Vec<_> = url.path_segments().into_iter().flatten().collect();
        let channel = match parts.as_slice() {
            ["archives" | "messages", channel, ..] => Some(*channel),
            ["client", _, channel, ..] => Some(*channel),
            _ => None,
        };
        let Some(channel) = channel.filter(|id| id.starts_with(['C', 'G'])) else {
            continue;
        };
        add(channel.into(), None);
        let thread = url
            .query_pairs()
            .find(|(key, _)| key == "thread_ts")
            .map(|(_, ts)| ts.into_owned())
            .or_else(|| {
                parts.iter().find_map(|part| {
                    let digits = part.strip_prefix('p')?;
                    (digits.len() == 16 && digits.bytes().all(|b| b.is_ascii_digit()))
                        .then(|| format!("{}.{}", &digits[..10], &digits[10..]))
                })
            });
        if let Some(thread) = thread {
            add(channel.into(), Some(thread));
        }
    }
    for id in IDS
        .get_or_init(|| Regex::new(r"\b[CG][A-Z0-9]{7,30}\b").unwrap())
        .find_iter(body)
    {
        add(id.as_str().into(), None);
    }
    for name in NAMES
        .get_or_init(|| Regex::new(r"(?:^|[\s`(<])#([a-z0-9][a-z0-9_-]{1,79})").unwrap())
        .captures_iter(body)
    {
        add(name[1].into(), None);
    }
    targets
}

struct Post {
    at: DateTime<Utc>,
    author: String,
    channel: String,
    id: String,
    text: String,
}

fn enrich(
    context: &str,
    handoff: &Entry,
    bots: &[String],
    mut read: impl FnMut(Value) -> Result<Value, String>,
) -> String {
    let targets = targets(&references(handoff));
    let mut incomplete = targets.len() > MAX_READS;
    let mut posts = BTreeMap::new();
    for target in targets.iter().take(MAX_READS) {
        let mut args = json!({
            "channel":target.channel, "since":handoff.created.to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
            "include_threads":true, "include_files":false, "include_channels":false,
            "max_messages":50, "max_bytes":65536,
        });
        if let Some(thread) = &target.thread {
            args["thread_id"] = json!(thread);
        }
        let Ok(receipt) = read(args) else {
            incomplete = true;
            break;
        };
        if receipt["ok"] != true {
            incomplete = true;
            break;
        }
        incomplete |= receipt["complete"] != true
            || receipt["cursor"].as_str().is_some_and(|c| !c.is_empty())
            || receipt["threads_failed"]
                .as_array()
                .is_some_and(|items| !items.is_empty());
        let Some(messages) = receipt["messages"].as_array() else {
            incomplete = true;
            continue;
        };
        incomplete |= messages.len() > 50;
        let channel = receipt
            .pointer("/channel/id")
            .and_then(Value::as_str)
            .or_else(|| receipt["channel"].as_str())
            .unwrap_or(&target.channel);
        for message in messages.iter().take(50) {
            let author = &message["author"];
            let Some(user) = author["id"]
                .as_str()
                .filter(|id| id.starts_with(['U', 'W']))
            else {
                continue;
            };
            if bots.iter().any(|bot| bot == user)
                || author["is_bot"] == true
                || author["type"] == "bot"
                || message["subtype"] == "bot_message"
                || message["bot_id"].is_string()
            {
                continue;
            }
            let at = message["created_at"]
                .as_str()
                .and_then(|at| DateTime::parse_from_rfc3339(at).ok());
            let (Some(at), Some(id), Some(text)) =
                (at, message["message_id"].as_str(), message["text"].as_str())
            else {
                incomplete = true;
                continue;
            };
            if at <= handoff.created {
                continue;
            }
            let name = ["display_name", "name", "real_name", "id"]
                .iter()
                .find_map(|key| author[*key].as_str().filter(|s| !s.trim().is_empty()))
                .unwrap_or(user);
            posts
                .entry((channel.to_string(), id.to_string()))
                .or_insert_with(|| Post {
                    at: at.with_timezone(&Utc),
                    author: excerpt(name, 30),
                    channel: excerpt(channel, 40),
                    id: id.into(),
                    text: text.into(),
                });
        }
    }
    let mut posts: Vec<_> = posts.into_values().collect();
    posts.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.id.cmp(&b.id)));
    let mut lines = Vec::new();
    let mut bytes = HEADING.len() + 256;
    for post in posts.iter().rev().take(MAX_POSTS) {
        let line = format!(
            "- {} · {} · {} · {}: {}",
            post.channel,
            post.author,
            post.at.format("%m-%d %H:%MZ"),
            post.id,
            excerpt(&post.text, 160)
        );
        if bytes + line.len() > SECTION_BYTES {
            break;
        }
        bytes += line.len() + 1;
        lines.push(line);
    }
    lines.reverse();
    if posts.len() > lines.len() {
        lines.push(format!(
            "+{} more newer human posts (read through Violet).",
            posts.len() - lines.len()
        ));
    }
    if incomplete {
        lines.push(COULD_NOT_CHECK.into());
    }
    if lines.is_empty() {
        return context.into();
    }
    insert_section(context, &format!("{HEADING}\n\n{}", lines.join("\n")))
}

fn excerpt(text: &str, max: usize) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = normalized.chars();
    let mut short: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        short.push('…');
    }
    short
}

fn insert_section(context: &str, section: &str) -> String {
    let Some(index) = context.find(HANDOFF) else {
        return context.into();
    };
    format!("{}{section}\n\n{}", &context[..index], &context[index..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_core::hooks::{
        ContextStores, DefaultHooksConfig, HookInput, build_context_with_stores,
    };
    use cas_store::{SqliteStore, Store};
    use serde_json::json;

    #[test]
    fn session_start_lists_posts_newer_than_named_handoff_and_omits_quiet_section() {
        let temp = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(temp.path()).unwrap();
        store.init().unwrap();
        let handoff = Entry {
            id: "handoff-new".into(),
            content:
                "Resume https://example.slack.com/archives/C123/p1793712000000000 in #project-chat"
                    .into(),
            created: "2026-11-03T13:20:00Z".parse().unwrap(),
            tags: vec!["handoff".into(), "role:supervisor".into()],
            ..Default::default()
        };
        let mut older = handoff.clone();
        older.id = "handoff-old".into();
        older.created -= chrono::Duration::hours(1);
        older.content = "Stale #wrong-channel".into();
        store.add(&older).unwrap();
        store.add(&handoff).unwrap();
        let input = HookInput {
            hook_event_name: "SessionStart".into(),
            agent_role: Some("supervisor".into()),
            ..Default::default()
        };
        let context = build_context_with_stores(
            &input,
            &ContextStores {
                project_store: Some(&store),
                ..ContextStores::empty()
            },
            &DefaultHooksConfig::new(),
            10,
            None,
            "mcp__cs__",
        )
        .unwrap()
        .0;
        let message = |id, time, text| {
            json!({
                "message_id":id, "created_at":time, "text":text,
                "author":{"id":"U123","display_name":"Operator","is_bot":false},
                "permalink":format!("https://example.slack.com/archives/C123/p{id}")
            })
        };
        let posts = vec![
            message("1", "2026-11-03T13:20:00Z", "At the handoff boundary"),
            message("2", "2026-11-03T13:35:00Z", "Check the channel too"),
            message("3", "2026-11-03T13:49:00Z", "The plan changed"),
        ];
        let mut channel_reads = 0;
        let updated = enrich(&context, &handoff, &[], |args| {
            assert_eq!(args["since"], "2026-11-03T13:20:00.000000Z");
            assert_ne!(args["channel"], "wrong-channel");
            if args.get("thread_id").is_none() {
                channel_reads += 1;
            }
            assert_eq!(args["include_files"], false);
            Ok(json!({"ok":true,"complete":true,"channel":{"id":"C123"},"messages":posts}))
        });
        assert!(
            channel_reads > 0,
            "thread references must also read sibling channel posts"
        );
        assert_eq!(handoff_id(&context).as_deref(), Some("handoff-new"));
        assert!(updated.contains("Operator"));
        let saved = updated.find("## 🔁 Current Handoff").unwrap();
        for text in ["Check the channel too", "The plan changed"] {
            assert!(
                updated.find(text).is_some_and(|pos| pos < saved),
                "{updated}"
            );
        }
        assert!(!updated.contains("At the handoff boundary"));
        assert_eq!(
            updated.matches("The plan changed").count(),
            1,
            "channel/thread overlap is deduplicated"
        );
        let quiet = enrich(&context, &handoff, &[], |_| {
            Ok(json!({"ok":true,"complete":true,"messages":[]}))
        });
        assert_eq!(quiet, context, "quiet channels add no section");
    }

    #[test]
    fn bounded_read_caps_human_posts_and_reports_partial_or_failed_checks() {
        let handoff = Entry {
            content: "#one #two #three #four #five #six #seven".into(),
            created: "2026-11-03T13:20:00Z".parse().unwrap(),
            ..Default::default()
        };
        let context = "## 🔁 Current Handoff\n\n### handoff-new — saved time\nbody";
        let mut messages: Vec<_> = (0..12)
            .map(|i| {
                json!({
                    "message_id":i.to_string(),"created_at":"2026-11-03T13:35:00Z",
                    "text":format!("human {i}"),"author":{"id":"U123","name":"A human"}
                })
            })
            .collect();
        for (id, author, subtype) in [
            ("app", json!({"id":null}), ""),
            ("bot", json!({"id":"U456","is_bot":true}), ""),
            ("configured-bot", json!({"id":"U789"}), ""),
            ("bot-subtype", json!({"id":"U456"}), "bot_message"),
        ] {
            messages.push(json!({"message_id":id,"created_at":"2026-11-03T13:35:00Z",
                "text":"excluded bot or app", "author":author,"subtype":subtype}));
        }
        let mut reads = 0;
        let capped = enrich(context, &handoff, &["U789".into()], |args| {
            reads += 1;
            assert_eq!(args["max_messages"], 50);
            Ok(json!({"ok":true,"complete":true,"channel":{"id":"C123"},"messages":messages}))
        });
        assert_eq!(reads, 6);
        assert_eq!(capped.matches("A human").count(), 10);
        assert!(capped.contains("+2 more"));
        assert!(!capped.contains("excluded bot or app"));
        assert!(capped.contains(COULD_NOT_CHECK));
        assert!(capped.len() - context.len() <= SECTION_BYTES);
        for result in [
            Err("Cloud unavailable".into()),
            Ok(json!({"ok":false})),
            Ok(json!({"ok":true,"complete":false,"messages":[]})),
        ] {
            let updated = enrich(context, &handoff, &[], |_| result.clone());
            assert!(updated.find(COULD_NOT_CHECK).unwrap() < updated.find(HANDOFF).unwrap());
            assert!(updated.ends_with(context));
        }
        let (release, wait) = mpsc::channel();
        let result: Result<(), ()> = bounded(Duration::ZERO, move || wait.recv().map_err(|_| ()));
        assert_eq!(
            result,
            Err(()),
            "a stalled Cloud/Violet boundary cannot hold startup"
        );
        release.send(()).unwrap();
    }
}
