use crate::hooks::handlers::*;

/// Build the per-turn supervisor reminder (≤512 bytes).
///
/// Emitted by `handle_user_prompt_submit` when `is_factory_agent && role==supervisor`.
/// Refreshes Hard Rules every turn to prevent mid-session drift where supervisors
/// forget their role and start using incorrect tools (e.g. `AskUserQuestion` for workers,
/// or `SendMessage` instead of coordination).
///
/// Identity is resolved at call time from process env vars set by the factory daemon:
/// - `CAS_AGENT_NAME` → supervisor name (e.g. "loyal-bear-96")
/// - `CAS_FACTORY_SESSION` → team/session name (e.g. "cas-src-wild-dragon-82")
///
/// EPIC cas-8888 (cas-fd9f): the coordination tool mention was hardcoded to
/// Claude's `mcp__cas__` prefix — this reminder fires every turn for every
/// factory supervisor, so a Grok (or Codex) supervisor was told a tool call
/// it can't make on every single turn. Now uses `harness_policy::own_tool_prefix()`.
pub(crate) fn build_supervisor_reminder() -> String {
    let name = std::env::var("CAS_AGENT_NAME").unwrap_or_else(|_| "supervisor".to_string());
    let team = std::env::var("CAS_FACTORY_SESSION").unwrap_or_else(|_| "team".to_string());
    let prefix = crate::harness_policy::own_tool_prefix();
    format!(
        "[supervisor reminder]\n\
         You are {name}, supervisor of team {team}. Hard rules:\n\
         \u{2022} AskUserQuestion is BLOCKED in factory mode: ask humans in plain text and end your turn; teammates \u{2192} {prefix}coordination action=message.\n\
         \u{2022} Never SendMessage. Never close worker tasks. Never implement non-trivial work yourself.\n\
         \u{2022} Never poll/monitor/sleep. End your turn after assigning. Wait for messages."
    )
}

/// Combine the factory turn-start context with whatever the normal capture
/// path produced (cas-7a01).
///
/// The factory block goes first: it is the only part of the output that can
/// carry an instruction the agent has not seen yet.
fn merge_prompt_context(factory_context: String, base: HookOutput) -> HookOutput {
    match base.user_prompt_context() {
        Some(existing) if !existing.trim().is_empty() => {
            HookOutput::with_user_prompt_context(format!("{factory_context}\n\n{existing}"))
        }
        _ => HookOutput::with_user_prompt_context(factory_context),
    }
}

pub fn handle_user_prompt_submit(
    input: &HookInput,
    cas_root: Option<&Path>,
) -> Result<HookOutput, MemError> {
    // === FACTORY TURN-START CONTEXT (cas-55ac reminder + cas-7a01 surfacing) ===
    //
    // cas-7a01 (GH #155): this is the *only* place in Cassy that reads the
    // prompt queue back and puts it in front of a recipient. Before it existed
    // the daemon's delivery had no counterpart, so a non-urgent message to an
    // idle Claude worker was written to an inbox file and never surfaced —
    // across turns the worker took.
    //
    // The supervisor reminder now APPENDS rather than returning early. Its
    // early return was a second, quieter half of the same bug: it made the
    // supervisor the one factory role whose mail could never be surfaced here,
    // because the handler returned before any queue read could happen.
    //
    // cas-78d3 (GH #165): this block now runs BEFORE the empty-prompt early
    // return, which used to sit at the top of this function. That ordering was
    // the third and worst instance of the same mistake: an unrelated capture
    // precondition ("is this prompt worth recording?") silently decided
    // whether a factory agent's mail got delivered at all. Combined with the
    // `prompt` vs `user_prompt` field-name mismatch it made the answer "no" on
    // every real Claude turn, which is why `acked_via = 'hook_surfaced'` had
    // never once been written by a live session. Surfacing must not depend on
    // the prompt: a blank or absent prompt is still a turn the recipient is
    // taking, and mail withheld from it is mail withheld indefinitely.
    let is_factory = crate::harness_policy::is_factory_agent(input);
    let is_supervisor = is_factory && crate::harness_policy::is_supervisor(input);
    let mut factory_context = String::new();
    if is_supervisor {
        factory_context.push_str(&build_supervisor_reminder());
    }
    if is_factory {
        if let Some(mail) = super::factory_inbox::surface_factory_inbox(cas_root, input) {
            if !factory_context.is_empty() {
                factory_context.push_str("\n\n");
            }
            factory_context.push_str(&mail);
        }
        if let Some(root) = cas_root {
            if let Some(packet) = crate::ambient_recall::build_ambient_recall_context(
                input,
                root,
                input.submitted_prompt(),
                false,
            ) {
                if !factory_context.is_empty() {
                    factory_context.push_str("\n\n");
                }
                factory_context.push_str(&packet.full);
            }
        }
    }
    if is_supervisor {
        // Supervisors still skip attribution capture — they don't write code.
        return Ok(HookOutput::with_user_prompt_context(factory_context));
    }

    // Attribution capture genuinely needs prompt text; surfacing did not.
    // Now that the two are ordered correctly, an absent prompt only skips the
    // half that cannot work without one.
    let base = match input.submitted_prompt() {
        Some(prompt_text) => handle_user_prompt_submit_capture(input, cas_root, prompt_text)?,
        None => HookOutput::empty(),
    };
    if factory_context.is_empty() {
        return Ok(base);
    }
    Ok(merge_prompt_context(factory_context, base))
}

/// Attribution capture + context/preference extraction: the historical body of
/// [`handle_user_prompt_submit`], unchanged. Split out (cas-7a01) so the
/// factory turn-start context can be merged into whatever it returns instead
/// of every early `HookOutput::empty()` silently discarding surfaced mail.
fn handle_user_prompt_submit_capture(
    input: &HookInput,
    cas_root: Option<&Path>,
    prompt_text: &str,
) -> Result<HookOutput, MemError> {
    // Check if Cassy is initialized (needed for all operations)
    let cas_root = match cas_root {
        Some(root) => root,
        None => return Ok(HookOutput::empty()),
    };

    // === ATTRIBUTION: Capture ALL prompts (even short ones) ===
    // This enables git blame-style attribution: tracing code back to prompts.
    // We capture all prompts because any prompt could trigger code changes.
    capture_prompt_for_attribution(cas_root, input, prompt_text);

    // Skip very short prompts for context/preference extraction
    // (acknowledgments like "yes", "ok" don't need context entries)
    if prompt_text.len() < 20 {
        return Ok(HookOutput::empty());
    }

    // Check if this prompt seems important enough to capture as context.
    let is_important = is_important_prompt(prompt_text);

    if !is_important {
        return Ok(HookOutput::empty());
    }

    // Prompt attribution above retains every user request in the dedicated
    // prompt store.  The memory store is an injection surface, so it needs a
    // stricter gate: conversational questions and recursive control-plane
    // prompts do not earn a durable context entry merely by containing a
    // question word or being very long.
    // Machine relay metadata reaches this hook as a typed HookInput field, not
    // by re-parsing a rendered `Cassy provenance:` prefix.  Absence is the
    // explicit operator case: a factory worker's genuine instruction must be
    // retained while every relay origin stays out of durable Context.
    let operator_submitted = matches!(
        input.submitted_prompt_origin(),
        cas_core::hooks::types::SubmittedPromptOrigin::Operator
    );
    if operator_submitted && is_context_memory_worthy_prompt(prompt_text) {
        if let Ok(store) = open_store(cas_root) {
            if let Ok(id) = store.generate_id() {
                let entry = Entry {
                    id,
                    entry_type: EntryType::Context,
                    content: format!("User request: {prompt_text}"),
                    tags: vec!["user-prompt".to_string()],
                    session_id: Some(input.session_id.clone()),
                    importance: 0.6, // User prompts are moderately important
                    ..Default::default()
                };

                // Store silently - don't fail the hook if storage fails.
                let _ = store.add(&entry);
            }
        }
    }

    // Check if this prompt expresses a preference/rule that should be remembered
    if is_preference_prompt(prompt_text) {
        // Try to extract and create a rule from the preference
        if let Some(preference) = extract_preference_from_prompt(prompt_text) {
            // Create a rule from the extracted preference
            if let Ok(rule_store) = crate::store::open_rule_store(cas_root) {
                let rule_id = rule_store
                    .generate_id()
                    .unwrap_or_else(|_| "rule-auto".to_string());
                let scope = if preference.scope == "global" {
                    crate::types::Scope::Global
                } else {
                    crate::types::Scope::Project
                };

                let rule = crate::types::Rule {
                    id: rule_id.clone(),
                    content: preference.content.clone(),
                    scope,
                    status: crate::types::RuleStatus::Draft,
                    paths: preference.path_pattern.unwrap_or_default(),
                    tags: vec!["auto-extracted".to_string()],
                    helpful_count: 0,
                    harmful_count: 0,
                    created: chrono::Utc::now(),
                    source_ids: vec![],
                    last_accessed: None,
                    review_after: None,
                    hook_command: None,
                    category: crate::types::RuleCategory::General,
                    priority: 2, // Normal priority
                    surface_count: 0,
                    auto_approve_tools: None,
                    auto_approve_paths: None,
                    team_id: None,
                    share: None,
                };

                if rule_store.add(&rule).is_ok() {
                    // Return context to inform the user about the created rule.
                    // EPIC cas-8888 (cas-fd9f): own_tool_prefix() — was
                    // hardcoded to mcp__cas__.
                    let prefix = crate::harness_policy::own_tool_prefix();
                    let msg = format!(
                        "\n<system-reminder>\n📝 Auto-detected preference from your message: \"{}\"\n   Created rule {} (scope: {}). Use `{prefix}rule action=helpful id={}` to confirm and promote to active.\n</system-reminder>",
                        preference.content, rule_id, preference.scope, rule_id
                    );
                    return Ok(HookOutput::with_user_prompt_context(msg));
                }
            }
        }
    }

    // Silent success
    Ok(HookOutput::empty())
}

/// Capture a prompt for code attribution (git blame for AI sessions)
///
/// This function stores every user prompt in the prompts table, enabling
/// later attribution: "which prompt created this code?"
///
/// Unlike the context entry logic, this captures ALL prompts (even short ones)
/// because any prompt could trigger code changes.
pub fn capture_prompt_for_attribution(
    cas_root: &std::path::Path,
    input: &HookInput,
    prompt_text: &str,
) {
    // Try to open the prompt store
    let store = match open_prompt_store(cas_root) {
        Ok(s) => s,
        Err(_) => return, // Silent failure - don't break the hook
    };

    // Use session_id-based agent ID for attribution
    let agent_id = current_agent_id(input);

    // Get current task ID from the task store (if any task is in_progress)
    let task_id = get_current_task_id(cas_root);

    // Create the prompt entry
    let prompt = Prompt::with_task(
        generate_prompt_id(),
        input.session_id.clone(),
        agent_id,
        prompt_text.to_string(),
        task_id,
    );

    // Store silently - attribution is best-effort
    let _ = store.add(&prompt);
}

/// Generate a unique prompt ID using ULID
pub fn generate_prompt_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    // Simple ULID-like format: prompt-{timestamp_hex}-{random_hex}
    let random: u32 = rand::random();
    format!("prompt-{:x}-{:04x}", timestamp, random & 0xFFFF)
}

/// Get the ID of the currently in-progress task (if any)
pub fn get_current_task_id(cas_root: &std::path::Path) -> Option<String> {
    let store = open_task_store(cas_root).ok()?;

    // Get tasks that are in_progress
    let tasks = store.list(Some(TaskStatus::InProgress)).ok()?;

    // Return the first in-progress task (most likely the one being worked on)
    tasks.into_iter().next().map(|t| t.id)
}

/// Capture the conversation transcript and update prompts with messages (blame v2)
///
/// Called at session end to store the full conversation for each prompt,
/// enabling rich context display in `cas blame --verbose`.
pub fn capture_transcript_for_prompts(cas_root: &std::path::Path, input: &HookInput) {
    // Skip if no transcript path available
    let transcript_path = match &input.transcript_path {
        Some(path) => std::path::Path::new(path),
        None => return,
    };

    // Parse transcript into messages
    let messages = match crate::hooks::transcript::parse_transcript_to_messages(transcript_path) {
        Ok(msgs) if !msgs.is_empty() => msgs,
        _ => return, // No messages or parse error - silent failure
    };

    // Extract model and tool version from transcript
    let metadata =
        crate::hooks::transcript::extract_transcript_metadata(transcript_path).unwrap_or_default();

    // Open prompt store
    let prompt_store = match open_prompt_store(cas_root) {
        Ok(store) => store,
        Err(_) => return,
    };

    // Get all prompts for this session
    let prompts = match prompt_store.list_by_session(&input.session_id, 100) {
        Ok(p) => p,
        Err(_) => return,
    };

    if prompts.is_empty() {
        return;
    }

    // Group messages by their corresponding prompts based on user message content matching
    // We'll associate all messages with the last prompt (simplest approach)
    if let Some(last_prompt) = prompts.first() {
        // Update the prompt with messages and metadata
        if prompt_store
            .update_blame_fields(
                &last_prompt.id,
                &messages,
                metadata.model.as_deref(),
                metadata.tool_version.as_deref(),
            )
            .is_ok()
        {
            let model_info = metadata
                .model
                .as_ref()
                .map(|m| format!(", model: {m}"))
                .unwrap_or_default();
            eprintln!(
                "cas: Captured {} conversation messages for prompt {}{}",
                messages.len(),
                &last_prompt.id[..16.min(last_prompt.id.len())],
                model_info
            );
        }
    }
}

/// Determine if a user prompt is important enough to capture
pub fn is_important_prompt(prompt: &str) -> bool {
    let prompt_lower = prompt.to_lowercase();

    // Task indicators - starting a new task
    let task_indicators = [
        "implement",
        "create",
        "add",
        "fix",
        "update",
        "refactor",
        "build",
        "write",
        "design",
        "help me",
        "i need",
        "please",
        "could you",
        "can you",
    ];

    // Question indicators
    let question_indicators = [
        "how",
        "what",
        "why",
        "where",
        "when",
        "which",
        "should",
        "would",
        "is there",
        "are there",
        "?",
    ];

    // Check for task indicators
    for indicator in task_indicators.iter() {
        if prompt_lower.contains(indicator) {
            return true;
        }
    }

    // Check for question indicators
    for indicator in question_indicators.iter() {
        if prompt_lower.contains(indicator) {
            return true;
        }
    }

    // Capture longer prompts (likely detailed instructions)
    if prompt.len() > 100 {
        return true;
    }

    false
}

/// Whether an otherwise-important prompt deserves a *memory* entry.
///
/// Prompt attribution is deliberately broader and keeps every prompt in its
/// own non-injected store. This gate protects the memory/ambient-recall path
/// from transient chat, recursive prompt-analysis payloads, and a single
/// oversized request that could consume a whole context budget.
pub fn is_context_memory_worthy_prompt(prompt: &str) -> bool {
    const MAX_CONTEXT_MEMORY_PROMPT_BYTES: usize = 12_000;
    const RECURSIVE_PREFERENCE_ANALYSIS: &str = "analyze this user prompt and determine if it expresses a coding preference or rule that should be remembered";

    if prompt.len() > MAX_CONTEXT_MEMORY_PROMPT_BYTES
        || prompt
            .to_lowercase()
            .contains(RECURSIVE_PREFERENCE_ANALYSIS)
    {
        return false;
    }

    let prompt_lower = prompt.to_lowercase();
    let task_indicators = [
        "implement",
        "create",
        "add",
        "fix",
        "update",
        "refactor",
        "build",
        "write",
        "design",
        "help me",
        "i need",
        "please",
        "could you",
        "can you",
    ];
    let has_task_indicator = task_indicators
        .iter()
        .any(|indicator| prompt_lower.contains(indicator));

    // A short question like "hello, what directory are you working in?" has
    // no durable recall value. The source prompt remains available for blame.
    prompt.len() >= 120 || has_task_indicator
}

/// Detect if a user prompt expresses a preference or rule
///
/// Looks for patterns like:
/// - "never add TODOs" / "don't add TODOs"
/// - "always implement full functionality"
/// - "prefer X over Y" / "use X instead of Y"
pub fn is_preference_prompt(prompt: &str) -> bool {
    let prompt_lower = prompt.to_lowercase();

    // Too short to be a meaningful preference
    if prompt.len() < 15 {
        return false;
    }

    // Preference indicators - patterns that suggest a rule/preference
    let preference_patterns = [
        // Negative patterns
        "never ",
        "don't ",
        "dont ",
        "do not ",
        "don't ever",
        "avoid ",
        "stop ",
        // Positive patterns
        "always ",
        "prefer ",
        "i prefer ",
        "use ",
        // Comparative patterns
        " instead of ",
        " over ",
        " rather than ",
    ];

    // Check for preference indicators
    for pattern in preference_patterns.iter() {
        if prompt_lower.contains(pattern) {
            // Ensure there's meaningful content after the keyword
            // (not just "never" or "always" alone)
            if let Some(pos) = prompt_lower.find(pattern) {
                let after = &prompt_lower[pos + pattern.len()..];
                if after.trim().len() >= 5 {
                    return true;
                }
            }
        }
    }

    false
}

/// Extract a preference/rule from a user prompt using AI
///
/// Returns None if no clear preference is detected or extraction fails
pub fn extract_preference_from_prompt(prompt: &str) -> Option<ExtractedPreference> {
    use std::time::Duration;
    use tokio::runtime::Runtime;

    let rt = Runtime::new().ok()?;

    // 3 second timeout - we don't want to delay the user too much
    rt.block_on(async {
        tokio::time::timeout(
            Duration::from_secs(3),
            extract_preference_from_prompt_async(prompt),
        )
        .await
        .ok()? // Timeout -> Option
        .ok() // Result -> Option
    })
    .flatten() // Option<Option<T>> -> Option<T>
}

/// Async implementation of preference extraction using Haiku
async fn extract_preference_from_prompt_async(
    prompt: &str,
) -> Result<Option<ExtractedPreference>, MemError> {
    use crate::tracing::claude_wrapper::traced_prompt;
    use claude_rs::QueryOptions;

    let prompt_text = format!(
        r#"Analyze this user prompt and determine if it expresses a coding preference or rule that should be remembered.

## User Prompt
{prompt}

## What Counts as a Preference
- Explicit rules: "never add TODOs", "always implement fully"
- Style preferences: "use async/await instead of .then()"
- Workflow rules: "run tests before committing"
- Code patterns: "prefer functional components"

## What to IGNORE
- Task requests: "fix the bug", "add a button"
- Questions: "how does this work?"
- Acknowledgments: "yes", "ok", "thanks"
- One-time instructions for current task only

## Task
If this is a preference that should be remembered for future sessions, extract it as JSON:
- content: The rule in imperative form ("Never X", "Always Y", "Use X instead of Y")
- scope: "global" if it's a personal preference, "project" if it seems project-specific
- confidence: 0.7-1.0 based on how clear and intentional the preference is

Respond with JSON only, no markdown:
{{"content": "...", "scope": "global", "confidence": 0.9}}

If this is NOT a preference to remember, respond with: null"#
    );

    let result = traced_prompt(
        &prompt_text,
        QueryOptions::new().model("claude-haiku-4-5").max_turns(1),
        "preference_extraction",
    )
    .await
    .map_err(|e| MemError::Other(format!("Preference extraction failed: {e}")))?;

    let response_text = result.text().trim();

    // Check for null response
    if response_text == "null" || response_text.is_empty() {
        return Ok(None);
    }

    // Parse JSON response
    let json_str = response_text
        .find('{')
        .and_then(|start| {
            response_text
                .rfind('}')
                .map(|end| &response_text[start..=end])
        })
        .unwrap_or(response_text);

    match serde_json::from_str::<ExtractedPreference>(json_str) {
        Ok(pref) if pref.confidence >= 0.7 => Ok(Some(pref)),
        Ok(_) => Ok(None),  // Low confidence, ignore
        Err(_) => Ok(None), // Parse failed, ignore
    }
}
