//! Claude Code hook input/output types
//!
//! Defines the JSON structures for communicating with Claude Code hooks.

use serde::{Deserialize, Serialize};

/// Input received from Claude Code hooks via stdin
#[derive(Debug, Clone, Deserialize, Default)]
pub struct HookInput {
    /// Unique session identifier
    #[serde(default, alias = "sessionId")]
    pub session_id: String,

    /// Path to the transcript file
    #[serde(default, alias = "transcriptPath")]
    pub transcript_path: Option<String>,

    /// Current working directory
    #[serde(default)]
    pub cwd: String,

    /// Grok Build workspace root. Claude/Codex payloads do not currently
    /// include this field; retaining it lets hook handlers recognize a Grok
    /// envelope even outside factory mode, where no harness env vars exist.
    #[serde(default, rename = "workspaceRoot")]
    pub workspace_root: Option<String>,

    /// Permission mode (default, plan, acceptEdits, bypassPermissions)
    #[serde(default, alias = "permissionMode")]
    pub permission_mode: Option<String>,

    /// Hook event name
    #[serde(default, alias = "hookEventName")]
    pub hook_event_name: String,

    /// Tool name (PostToolUse)
    #[serde(default, alias = "toolName")]
    pub tool_name: Option<String>,

    /// Tool input parameters (PostToolUse)
    #[serde(default, alias = "toolInput")]
    pub tool_input: Option<serde_json::Value>,

    /// Tool response (PostToolUse)
    #[serde(default, alias = "toolResult")]
    pub tool_response: Option<serde_json::Value>,

    /// Tool use ID (PostToolUse)
    #[serde(default, alias = "toolUseId")]
    pub tool_use_id: Option<String>,

    /// Whether Grok Build truncated `toolInput` before dispatching the hook.
    /// Presence (including `false`) is a payload-shape marker; the value is
    /// otherwise informational for CAS today.
    #[serde(default, rename = "toolInputTruncated")]
    pub tool_input_truncated: Option<bool>,

    /// User prompt text (UserPromptSubmit).
    ///
    /// cas-78d3 (GH #165): Claude Code sends the submitted text under the key
    /// **`prompt`**, not `user_prompt`. Without that alias this field
    /// deserialized to `None` on every real turn, and `prompt` was instead
    /// swallowed by `subagent_prompt` — the only field that aliased it, and one
    /// with no readers anywhere in the tree. The visible consequence was that
    /// `handle_user_prompt_submit` returned empty before it could reach either
    /// attribution capture or the cas-7a01 turn-start inbox surfacing, so
    /// `acked_via = 'hook_surfaced'` was never written by a live session.
    ///
    /// Read this through [`HookInput::submitted_prompt`] rather than directly,
    /// so blank-vs-absent is handled in one place.
    #[serde(default, alias = "userPrompt", alias = "prompt")]
    pub user_prompt: Option<String>,

    /// Session start source (SessionStart)
    #[serde(default)]
    pub source: Option<String>,

    /// Session end reason (SessionEnd)
    #[serde(default)]
    pub reason: Option<String>,

    /// Subagent type — **corresponds to no observed wire key** (cas-f3e3).
    ///
    /// A live capture of `SubagentStart` / `SubagentStop` from Claude Code
    /// 2.1.224 shows the child's type arriving as [`Self::agent_type`]
    /// (`agent_type`), alongside `agent_id`. Neither `subagent_type` nor
    /// `subagentType` appears in either payload, so this field is `None` on
    /// every real event and must not be used to decide anything.
    ///
    /// Kept (rather than deleted) only because removing it would churn a large
    /// number of unrelated struct literals; the wire-shape test
    /// `subagentstart_payload_uses_agent_type_not_subagent_type` pins the fact
    /// so nobody re-derives a dependency on it. Read [`Self::agent_type`].
    ///
    /// NOTE: the `subagent_type` key that *does* exist on the wire is a member
    /// of the **`tool_input` object** for the `Task`/`Agent` tool, which is a
    /// different thing entirely and is read as JSON out of `tool_input`.
    #[serde(default, alias = "subagentType")]
    pub subagent_type: Option<String>,

    /// Loop-prevention signal for `Stop` / `SubagentStop` (cas-f3e3).
    ///
    /// Claude Code sends `stop_hook_active: true` when the session is **already
    /// continuing as the result of a stop hook** — i.e. a previous `Stop`
    /// returned `decision: "block"` and the model was told to keep working. The
    /// harness's documented contract is that a Stop hook must check this before
    /// blocking again, otherwise a blocker the model cannot clear by continuing
    /// loops forever.
    ///
    /// It was declared nowhere and read nowhere while CAS blocked `Stop` in
    /// five places, so CAS had no brake at all. Read this through
    /// [`HookInput::stop_hook_is_reentrant`] rather than directly.
    ///
    /// Confirmed on the wire (not just in docs) by a live capture: present on
    /// both `Stop` and `SubagentStop`.
    #[serde(default, alias = "stopHookActive")]
    pub stop_hook_active: Option<bool>,

    /// Distinct child identifier supplied by SubagentStart/SubagentStop.
    ///
    /// `session_id` remains the parent session for these events; authorization
    /// that must distinguish parent from child must use this field.
    #[serde(default, alias = "agentId")]
    pub agent_id: Option<String>,

    /// Current harness name for the spawned/stopped child.
    #[serde(default, alias = "agentType")]
    pub agent_type: Option<String>,

    /// Subagent prompt (SubagentStart).
    ///
    /// cas-78d3: the bare `prompt` alias moved to [`Self::user_prompt`], where
    /// Claude Code actually sends it. Nothing reads this field today, so the
    /// move costs no behaviour; leaving the alias here cost every turn's mail.
    #[serde(default, alias = "subagentPrompt")]
    pub subagent_prompt: Option<String>,

    /// CAS agent role for this hook invocation ("supervisor" / "worker") —
    /// populated by the harness at dispatch time from the process env var
    /// `CAS_AGENT_ROLE`. Kept as an explicit field on `HookInput` so hook
    /// handlers don't have to re-read process-global state at call time;
    /// this makes the contract explicit and future-proofs against any
    /// inline hook dispatch (e.g. from an MCP handler in `cas serve`) where
    /// env mutations from other MCP tools could race with the role read.
    ///
    /// Never sent from Claude Code on stdin — `#[serde(default)]` keeps
    /// deserialization of existing payloads unchanged.
    #[serde(default)]
    pub agent_role: Option<String>,

    /// Assistant text for `MessageDisplay`, and the notification text for
    /// `Notification`.
    ///
    /// cas-f3e3: **`MessageDisplay` does not send a `message` key.** A live
    /// capture from Claude Code 2.1.224 shows the payload as
    /// `{session_id, transcript_path, cwd, prompt_id, hook_event_name,
    /// turn_id, message_id, index, final, delta}` — the text arrives under
    /// **`delta`**, as one chunk of a stream. Without the alias below this
    /// field was `None` on every MessageDisplay event, so
    /// `handle_message_display` returned at its first line and the entire
    /// cas-97ba Ink-crash guard + secret redaction feature was unreachable —
    /// the same failure shape as GH #165, and invisible for the same reason
    /// (its tests build `HookInput` by struct literal).
    ///
    /// `Notification` genuinely uses `message`, so both spellings must work;
    /// the alias is additive.
    ///
    /// Read this through [`HookInput::display_text`], which also enforces the
    /// chunk-boundary rule documented on [`Self::message_is_final`].
    #[serde(default, alias = "delta")]
    pub message: Option<String>,

    /// `MessageDisplay`: true on the last chunk of an assistant message
    /// (cas-f3e3). Serialized as `final`, which is a Rust keyword.
    ///
    /// This matters because every transform in `handle_message_display`
    /// (nested-fence rewriting, secret redaction) reasons over a *whole*
    /// message: a fence opened in chunk N and closed in chunk N+2, or a token
    /// split across a chunk boundary, cannot be judged from one chunk. So the
    /// guard only acts on a chunk that is marked final.
    ///
    /// Absent for every other event, and treated as "final" when absent so a
    /// harness that omits it is not silently skipped.
    #[serde(default, rename = "final")]
    pub message_is_final: Option<bool>,

    /// `MessageDisplay`: zero-based index of this chunk within the assistant
    /// message identified by `message_id` (cas-f3e3). Informational; declared
    /// so the streaming shape is visible in the type rather than inferred.
    #[serde(default)]
    pub index: Option<u64>,
}

impl HookInput {
    /// The submitted prompt text for `UserPromptSubmit`, trimmed, `None` when
    /// absent or blank.
    ///
    /// cas-78d3 (GH #165): the single place that decides "is there a prompt".
    /// It exists because the previous inline `match &input.user_prompt` in
    /// `handle_user_prompt_submit` conflated "no prompt" with "nothing to do",
    /// and a field-name mismatch made "no prompt" the answer on every real
    /// turn. Callers that need to *do work regardless* of the prompt must not
    /// gate on this.
    pub fn submitted_prompt(&self) -> Option<&str> {
        self.user_prompt
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
    }

    /// True when this `Stop` / `SubagentStop` fired while the session was
    /// **already continuing because a previous stop hook blocked it**
    /// (cas-f3e3).
    ///
    /// A Stop handler that returns `decision: "block"` on a condition the model
    /// cannot clear by continuing will otherwise loop forever; this is the
    /// harness's only brake, and the single place its polarity is decided.
    /// Absent is treated as "not re-entrant", which preserves the behaviour of
    /// every harness that does not send the key.
    pub fn stop_hook_is_reentrant(&self) -> bool {
        self.stop_hook_active.unwrap_or(false)
    }

    /// The assistant text a `MessageDisplay` guard may safely transform, or
    /// `None` when there is nothing to inspect (cas-f3e3).
    ///
    /// `None` when the chunk carries no text, or when it is a non-final chunk
    /// of a streamed message — see [`Self::message_is_final`] for why a partial
    /// chunk cannot be judged. Blank text is `None` too, so callers cannot
    /// re-derive the blank-vs-absent conflation that caused GH #165.
    pub fn display_text(&self) -> Option<&str> {
        if self.message_is_final == Some(false) {
            return None;
        }
        self.message
            .as_deref()
            .filter(|text| !text.trim().is_empty())
    }
}

/// Output sent back to Claude Code via stdout (JSON)
#[derive(Debug, Clone, Serialize, Default)]
pub struct HookOutput {
    /// Whether to continue (false stops Claude entirely)
    #[serde(skip_serializing_if = "Option::is_none", rename = "continue")]
    pub continue_session: Option<bool>,

    /// Reason for stopping (when continue=false, shown to user not Claude)
    #[serde(skip_serializing_if = "Option::is_none", rename = "stopReason")]
    pub stop_reason: Option<String>,

    /// Decision control for Stop/SubagentStop/PostToolUse hooks
    /// - "block" prevents the action (for Stop: Claude continues working)
    /// - undefined allows the action
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,

    /// Reason for decision (shown to Claude when decision="block")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,

    /// Suppress output from transcript
    #[serde(skip_serializing_if = "Option::is_none", rename = "suppressOutput")]
    pub suppress_output: Option<bool>,

    /// System message to show user
    #[serde(skip_serializing_if = "Option::is_none", rename = "systemMessage")]
    pub system_message: Option<String>,

    /// Hook-specific output
    #[serde(skip_serializing_if = "Option::is_none", rename = "hookSpecificOutput")]
    pub hook_specific_output: Option<HookSpecificOutput>,
}

/// Hook-specific output — tagged by the event it belongs to.
///
/// Each variant models one of the events that Claude Code's schema permits on
/// `hookSpecificOutput`. Events that the schema *forbids* here
/// (Stop / SubagentStop / PreCompact / SessionEnd / SubagentStart /
/// Notification) intentionally have NO variant — it is a compile-time error
/// to construct one. Those events route context through
/// `HookOutput::system_message` instead.
///
/// The doc-tests below are the type-system regression guard. They use the
/// `Variant { .. }` shape so a compile failure can ONLY mean "no variant named
/// X" — a false pass via wrong-field-name is not possible. (rustdoc
/// compile_fail only asserts *some* compile error fires; the shape below
/// leaves no other failure mode.)
///
/// ```compile_fail
/// use cas_core::hooks::HookSpecificOutput;
/// let _: HookSpecificOutput = HookSpecificOutput::Stop { .. };
/// ```
///
/// ```compile_fail
/// use cas_core::hooks::HookSpecificOutput;
/// let _: HookSpecificOutput = HookSpecificOutput::SubagentStop { .. };
/// ```
///
/// ```compile_fail
/// use cas_core::hooks::HookSpecificOutput;
/// let _: HookSpecificOutput = HookSpecificOutput::PreCompact { .. };
/// ```
///
/// ```compile_fail
/// use cas_core::hooks::HookSpecificOutput;
/// let _: HookSpecificOutput = HookSpecificOutput::SessionEnd { .. };
/// ```
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "hookEventName")]
pub enum HookSpecificOutput {
    PreToolUse {
        #[serde(rename = "permissionDecision", skip_serializing_if = "Option::is_none")]
        permission_decision: Option<String>,
        #[serde(
            rename = "permissionDecisionReason",
            skip_serializing_if = "Option::is_none"
        )]
        permission_decision_reason: Option<String>,
        #[serde(rename = "updatedInput", skip_serializing_if = "Option::is_none")]
        updated_input: Option<serde_json::Value>,
        /// Optional context injected alongside the tool result (Claude Code
        /// PreToolUse supports this). Used by factory SendMessage auto-route
        /// to surface a success receipt without a deny/`<error>` envelope
        /// (cas-73c8).
        #[serde(rename = "additionalContext", skip_serializing_if = "Option::is_none")]
        additional_context: Option<String>,
    },
    UserPromptSubmit {
        /// Required by Claude Code's schema — a UserPromptSubmit
        /// hookSpecificOutput without `additionalContext` is rejected.
        #[serde(rename = "additionalContext")]
        additional_context: String,
    },
    PostToolUse {
        #[serde(rename = "additionalContext", skip_serializing_if = "Option::is_none")]
        additional_context: Option<String>,
    },
    SessionStart {
        #[serde(rename = "additionalContext")]
        additional_context: String,
        /// Ask the session to re-scan skill dirs and reload skill markdown without
        /// a daemon restart. Emitted when `cas update --sync` has written new skill
        /// content since this session last loaded skills.
        ///
        /// Absent (None) when no drift is detected — `skip_serializing_if` keeps
        /// the field out of the JSON payload entirely in that case, matching
        /// Claude Code's expectation that unknown `false`-ish booleans may be
        /// omitted.
        #[serde(rename = "reloadSkills", skip_serializing_if = "Option::is_none")]
        reload_skills: Option<bool>,
        /// Human-readable session title surfaced in the Claude Code agent dashboard
        /// and factory tmux panes so each pane shows which worker owns which task.
        ///
        /// Workers: `[worker] <task-id> · <title preview>` or `[worker] idle`.
        /// Supervisors: `[supervisor] <epic-id>` or `[supervisor] factory`.
        /// Non-factory sessions: absent.
        ///
        /// Added by cas-ae09; absent when None to preserve unchanged wire shape
        /// for sessions that don't participate in factory mode.
        #[serde(rename = "sessionTitle", skip_serializing_if = "Option::is_none")]
        session_title: Option<String>,
    },
    PermissionRequest {
        #[serde(rename = "permissionDecision")]
        permission_decision: String,
        #[serde(
            rename = "permissionDecisionReason",
            skip_serializing_if = "Option::is_none"
        )]
        permission_decision_reason: Option<String>,
    },

    /// MessageDisplay hook (CC 2.1.152+) — replaces the assistant message text
    /// before it is rendered to the terminal. When `updated_message` is None
    /// the variant is serialized but carries no transform (prefer returning
    /// `HookOutput::empty()` for pure passthrough instead).
    ///
    /// Only emitted when the guard is opt-in (`[hooks] message_display_guard =
    /// true`) AND a transform is actually needed; benign content returns
    /// `HookOutput::empty()` so Claude Code does a zero-copy passthrough.
    MessageDisplay {
        #[serde(rename = "updatedMessage", skip_serializing_if = "Option::is_none")]
        updated_message: Option<String>,
    },
}

impl HookOutput {
    /// Create an empty output (success, no changes)
    pub fn empty() -> Self {
        Self::default()
    }

    // ---- Typed builders for hookSpecificOutput ---------------------------
    //
    // One builder per schema-valid (event, shape) pair. The string-keyed
    // `with_context` / `with_permission_decision` / `with_updated_input` that
    // existed before the enum refactor are intentionally gone: a runtime
    // string argument cannot be validated against the schema at the call site,
    // which is exactly the hole baa540b fell through. Each builder below is
    // callable only for events whose schema allows the shape it produces.

    /// UserPromptSubmit hookSpecificOutput — `additionalContext` is required.
    pub fn with_user_prompt_context(context: String) -> Self {
        Self {
            hook_specific_output: Some(HookSpecificOutput::UserPromptSubmit {
                additional_context: context,
            }),
            ..Default::default()
        }
    }

    /// The `additionalContext` this output injects at `UserPromptSubmit`, if
    /// any (cas-7a01).
    ///
    /// A `UserPromptSubmit` handler that wants to add context to whatever an
    /// inner step already produced has to be able to read it back — otherwise
    /// the only way to combine two pieces of turn-start context is for one to
    /// overwrite the other, which is how the factory supervisor's early-return
    /// reminder silently suppressed everything downstream of it.
    ///
    /// `None` for every other event shape.
    pub fn user_prompt_context(&self) -> Option<&str> {
        match &self.hook_specific_output {
            Some(HookSpecificOutput::UserPromptSubmit { additional_context }) => {
                Some(additional_context.as_str())
            }
            _ => None,
        }
    }

    /// PostToolUse hookSpecificOutput — `additionalContext` is optional.
    pub fn with_post_tool_context(context: String) -> Self {
        Self {
            hook_specific_output: Some(HookSpecificOutput::PostToolUse {
                additional_context: Some(context),
            }),
            ..Default::default()
        }
    }

    /// SessionStart hookSpecificOutput — `additionalContext` injects into the
    /// agent's context window.
    pub fn with_session_start_context(context: String) -> Self {
        Self {
            hook_specific_output: Some(HookSpecificOutput::SessionStart {
                additional_context: context,
                reload_skills: None,
                session_title: None,
            }),
            ..Default::default()
        }
    }

    /// Set `reloadSkills: true` on an existing `SessionStart` output, or create
    /// a minimal `SessionStart` output when the current output is empty.
    ///
    /// Has no effect when `reload` is `false` and there is no existing
    /// `SessionStart` output — this avoids accidentally emitting an empty
    /// `hookSpecificOutput` payload for non-SessionStart events.
    pub fn with_reload_skills(mut self, reload: bool) -> Self {
        match self.hook_specific_output {
            Some(HookSpecificOutput::SessionStart {
                ref mut reload_skills,
                ..
            }) => {
                *reload_skills = Some(reload);
            }
            None if reload => {
                // No existing output yet — emit a minimal SessionStart so
                // `reloadSkills` has a valid hookEventName wrapper.
                self.hook_specific_output = Some(HookSpecificOutput::SessionStart {
                    additional_context: String::new(),
                    reload_skills: Some(true),
                    session_title: None,
                });
            }
            _ => {}
        }
        self
    }

    /// Set `sessionTitle` on an existing `SessionStart` output, or create a
    /// minimal `SessionStart` output when the current output is empty (cas-ae09).
    ///
    /// Has no effect on non-SessionStart outputs.
    pub fn with_session_title(mut self, title: String) -> Self {
        match self.hook_specific_output {
            Some(HookSpecificOutput::SessionStart {
                ref mut session_title,
                ..
            }) => {
                *session_title = Some(title);
            }
            None => {
                self.hook_specific_output = Some(HookSpecificOutput::SessionStart {
                    additional_context: String::new(),
                    reload_skills: None,
                    session_title: Some(title),
                });
            }
            _ => {}
        }
        self
    }

    /// PreToolUse permission decision. `decision` must be `"allow"`, `"deny"`,
    /// or `"ask"` per Claude Code's schema. TODO(cas-e55b follow-up): tighten
    /// `decision: &str` to a typed `PermissionDecision` enum so invalid values
    /// fail to compile. Current callers all pass string literals so the
    /// migration is trivial; deferred from the enum refactor to keep that diff
    /// focused.
    pub fn with_pre_tool_permission(decision: &str, reason: &str) -> Self {
        Self {
            hook_specific_output: Some(HookSpecificOutput::PreToolUse {
                permission_decision: Some(decision.to_string()),
                permission_decision_reason: Some(reason.to_string()),
                updated_input: None,
                additional_context: None,
            }),
            ..Default::default()
        }
    }

    /// PreToolUse permission decision with `additionalContext` for Claude.
    ///
    /// Use when the hook both decides permission and needs to inject a
    /// success-shaped receipt into the model's context (e.g. factory
    /// SendMessage auto-route: `allow` + "AUTO-ROUTED" context so the tool
    /// is not reported as an `<error>`).
    pub fn with_pre_tool_permission_and_context(
        decision: &str,
        reason: &str,
        additional_context: &str,
    ) -> Self {
        Self {
            hook_specific_output: Some(HookSpecificOutput::PreToolUse {
                permission_decision: Some(decision.to_string()),
                permission_decision_reason: Some(reason.to_string()),
                updated_input: None,
                additional_context: Some(additional_context.to_string()),
            }),
            ..Default::default()
        }
    }

    /// PreToolUse with a modified tool input. Claude Code applies the updated
    /// input in place of the original before the tool runs.
    pub fn with_pre_tool_updated_input(updated_input: serde_json::Value) -> Self {
        Self {
            hook_specific_output: Some(HookSpecificOutput::PreToolUse {
                permission_decision: None,
                permission_decision_reason: None,
                updated_input: Some(updated_input),
                additional_context: None,
            }),
            ..Default::default()
        }
    }

    /// PermissionRequest decision — `decision` is `"allow"` / `"deny"` /
    /// `"ask"`.
    ///
    /// Note: `permissionDecision` appears both at the top level of HookOutput
    /// (universal schema field) and inside hookSpecificOutput for
    /// PermissionRequest. Claude Code reads the hookSpecificOutput form for
    /// PermissionRequest events; the top-level field is for other event
    /// surfaces. This builder writes the hookSpecificOutput form only, which
    /// matches historical behavior from before the enum refactor.
    pub fn with_permission_request(decision: &str, reason: &str) -> Self {
        Self {
            hook_specific_output: Some(HookSpecificOutput::PermissionRequest {
                permission_decision: decision.to_string(),
                permission_decision_reason: Some(reason.to_string()),
            }),
            ..Default::default()
        }
    }

    /// MessageDisplay transform — replaces the assistant message text before it
    /// reaches the terminal renderer. Only call this when a transform is actually
    /// needed; return `HookOutput::empty()` for passthrough (no allocation).
    pub fn with_message_display_transform(updated: String) -> Self {
        Self {
            hook_specific_output: Some(HookSpecificOutput::MessageDisplay {
                updated_message: Some(updated),
            }),
            ..Default::default()
        }
    }

    // ---- Non-hookSpecificOutput builders ---------------------------------

    /// Create output that injects context via `systemMessage` for events that
    /// don't support `hookSpecificOutput.additionalContext` (Stop, SubagentStop,
    /// PreCompact, SessionEnd).
    pub fn with_system_context(context: String) -> Self {
        Self {
            system_message: Some(context),
            ..Default::default()
        }
    }

    /// Create output that signals an error (exit code 2)
    pub fn blocking_error(message: String) -> Self {
        Self {
            system_message: Some(message),
            ..Default::default()
        }
    }

    /// Create output that blocks the Stop hook (Claude continues working)
    /// Use this when you want to prevent Claude from stopping.
    /// The reason is shown to Claude to explain why it should continue.
    pub fn block_stop(reason: String) -> Self {
        Self {
            decision: Some("block".to_string()),
            reason: Some(reason),
            ..Default::default()
        }
    }

    /// Create output that blocks Stop and also injects context.
    ///
    /// Stop-family events must route context through `systemMessage`, never
    /// through hookSpecificOutput. The typed enum makes the latter
    /// unrepresentable; this helper makes the former easy.
    pub fn block_stop_with_context(reason: String, context: String) -> Self {
        Self {
            decision: Some("block".to_string()),
            reason: Some(reason),
            system_message: Some(context),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::hooks::types::*;

    #[test]
    fn test_parse_session_start_input() {
        let json = r#"{
            "session_id": "abc123",
            "cwd": "/test/dir",
            "hook_event_name": "SessionStart",
            "source": "startup"
        }"#;

        let input: HookInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.session_id, "abc123");
        assert_eq!(input.hook_event_name, "SessionStart");
        assert_eq!(input.source, Some("startup".to_string()));
    }

    #[test]
    fn test_parse_post_tool_use_input() {
        let json = r#"{
            "session_id": "abc123",
            "cwd": "/test/dir",
            "hook_event_name": "PostToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": "/test/file.rs"},
            "tool_response": {"success": true}
        }"#;

        let input: HookInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.tool_name, Some("Write".to_string()));
        assert!(input.tool_input.is_some());
    }

    #[test]
    fn test_parse_grok_post_tool_use_input() {
        // Exact Grok Build command-hook envelope shape from the official
        // hooks guide: common and tool fields are camelCase, terminal calls
        // use run_terminal_command, and PostToolUse output is toolResult.
        let json = r#"{
            "hookEventName": "post_tool_use",
            "sessionId": "grok-session-123",
            "cwd": "/test/dir",
            "workspaceRoot": "/test/dir",
            "permissionMode": "default",
            "toolName": "run_terminal_command",
            "toolInput": {"command": "git commit -m 'grok work'"},
            "toolResult": {"exitCode": 0, "stdout": "[factory/grok abc1234] grok work"},
            "toolUseId": "tool-use-456",
            "toolInputTruncated": false,
            "timestamp": "2026-04-14T12:00:00Z"
        }"#;

        let input: HookInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.session_id, "grok-session-123");
        assert_eq!(input.hook_event_name, "post_tool_use");
        assert_eq!(input.workspace_root.as_deref(), Some("/test/dir"));
        assert_eq!(input.permission_mode.as_deref(), Some("default"));
        assert_eq!(input.tool_name.as_deref(), Some("run_terminal_command"));
        assert_eq!(
            input
                .tool_input
                .as_ref()
                .and_then(|value| value.get("command"))
                .and_then(|value| value.as_str()),
            Some("git commit -m 'grok work'")
        );
        assert_eq!(
            input
                .tool_response
                .as_ref()
                .and_then(|value| value.get("exitCode"))
                .and_then(|value| value.as_i64()),
            Some(0)
        );
        assert_eq!(input.tool_use_id.as_deref(), Some("tool-use-456"));
        assert_eq!(input.tool_input_truncated, Some(false));
    }

    #[test]
    fn test_hook_output_serialization() {
        let output = HookOutput::with_session_start_context("Test context".to_string());
        let json = serde_json::to_string(&output).unwrap();

        assert!(json.contains("hookSpecificOutput"));
        assert!(json.contains("SessionStart"));
        assert!(json.contains("additionalContext"));
    }

    #[test]
    fn test_empty_output() {
        let output = HookOutput::empty();
        let json = serde_json::to_string(&output).unwrap();
        assert_eq!(json, "{}");
    }

    #[test]
    fn test_continue_field_name() {
        let output = HookOutput {
            continue_session: Some(false),
            stop_reason: Some("test".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(
            json.contains("\"continue\""),
            "Expected 'continue' but got: {json}"
        );
        assert!(
            json.contains("\"stopReason\""),
            "Expected 'stopReason' but got: {json}"
        );
    }

    #[test]
    fn test_with_system_context_has_no_hook_specific_output() {
        // Stop / SubagentStop / PreCompact / SessionEnd must route context via
        // `systemMessage`, NOT `hookSpecificOutput.additionalContext` — the
        // latter is rejected by Claude Code's schema for these events and
        // causes the entire hook output to be discarded. Regression guard for
        // cas-8299.
        let output = HookOutput::with_system_context("codemap is stale".to_string());
        let json = serde_json::to_string(&output).unwrap();
        assert!(
            json.contains("\"systemMessage\":\"codemap is stale\""),
            "Expected systemMessage in output: {json}"
        );
        assert!(
            !json.contains("hookSpecificOutput"),
            "with_system_context must NOT emit hookSpecificOutput: {json}"
        );
        assert!(
            !json.contains("additionalContext"),
            "with_system_context must NOT emit additionalContext: {json}"
        );
    }

    #[test]
    fn test_block_stop_output() {
        let output = HookOutput::block_stop("Continue working on remaining tasks".to_string());
        let json = serde_json::to_string(&output).unwrap();
        assert!(
            json.contains("\"decision\":\"block\""),
            "Expected decision:block but got: {json}"
        );
        assert!(
            json.contains("\"reason\":\"Continue working"),
            "Expected reason but got: {json}"
        );
    }

    /// cas-78d3 (GH #165): Claude Code names this key `prompt`. CAS aliased
    /// `prompt` onto `subagent_prompt` instead, so `user_prompt` was `None` on
    /// every real turn — and every consumer downstream of it (attribution
    /// capture, turn-start inbox surfacing, `acked_via = 'hook_surfaced'`) was
    /// dead code in production for a full release while its unit tests, which
    /// all constructed `HookInput` by hand, stayed green.
    ///
    /// The lesson this pins: a struct-literal test cannot catch a
    /// deserialization-contract bug. Parse the real wire shape.
    #[test]
    fn userpromptsubmit_payload_deserializes_the_prompt_key() {
        let input: HookInput = serde_json::from_str(
            r#"{"session_id":"s","transcript_path":"/tmp/t","cwd":"/tmp",
                "hook_event_name":"UserPromptSubmit","prompt":"do the thing"}"#,
        )
        .expect("Claude's real payload must deserialize");

        assert_eq!(
            input.user_prompt.as_deref(),
            Some("do the thing"),
            "`prompt` must land in user_prompt, not be swallowed elsewhere"
        );
        assert_eq!(input.submitted_prompt(), Some("do the thing"));
        assert_eq!(
            input.subagent_prompt, None,
            "`prompt` must no longer be captured by subagent_prompt"
        );
    }

    /// The legacy/explicit spellings keep working, so nothing that was already
    /// reaching the handler stops doing so.
    #[test]
    fn userpromptsubmit_payload_still_accepts_legacy_spellings() {
        for body in [
            r#"{"hook_event_name":"UserPromptSubmit","user_prompt":"hi there"}"#,
            r#"{"hook_event_name":"UserPromptSubmit","userPrompt":"hi there"}"#,
        ] {
            let input: HookInput = serde_json::from_str(body).expect("must deserialize");
            assert_eq!(input.submitted_prompt(), Some("hi there"), "{body}");
        }
    }

    /// `submitted_prompt` is the single place blank-vs-absent is decided, so
    /// callers cannot re-derive the conflation that caused GH #165.
    #[test]
    fn submitted_prompt_treats_blank_and_absent_alike() {
        let blank: HookInput =
            serde_json::from_str(r#"{"hook_event_name":"UserPromptSubmit","prompt":"  \t "}"#)
                .unwrap();
        assert_eq!(blank.submitted_prompt(), None);

        let absent: HookInput =
            serde_json::from_str(r#"{"hook_event_name":"UserPromptSubmit"}"#).unwrap();
        assert_eq!(absent.submitted_prompt(), None);

        let padded: HookInput =
            serde_json::from_str(r#"{"hook_event_name":"UserPromptSubmit","prompt":"  hi  "}"#)
                .unwrap();
        assert_eq!(padded.submitted_prompt(), Some("hi"));
    }

    // ── cas-f3e3: captured-wire regression tests ──────────────────────────
    //
    // Every payload below is a VERBATIM capture from Claude Code 2.1.224,
    // taken by pointing a hook command at a script that appends stdin to a
    // file and then taking a real turn. They are not transcribed from docs and
    // not derived from these structs — that circularity is exactly what hid
    // GH #165 (and, as `messagedisplay_payload_carries_text_under_delta`
    // below shows, a second dead handler alongside it).

    /// MessageDisplay sends the assistant text as `delta`, NOT `message`.
    ///
    /// Before cas-f3e3 this payload left `input.message == None`, so
    /// `handle_message_display` returned on its first line for every real
    /// event and the Ink-crash guard / secret redaction could never run.
    #[test]
    fn messagedisplay_payload_carries_text_under_delta() {
        let input: HookInput = serde_json::from_str(
            r#"{"session_id":"fc70e6f3-4e32-43cc-b2da-259c777cd9a6",
                "transcript_path":"/tmp/wirecap/projects/x.jsonl",
                "cwd":"/tmp/wirecap/proj",
                "prompt_id":"d323a56b-73b1-4afc-8b3f-32605be51b91",
                "hook_event_name":"MessageDisplay",
                "turn_id":"83db6fbe-820e-4db4-8845-d5515e9158c3",
                "message_id":"513a75fd-35f5-451b-a7ed-b925c5ecc4d9",
                "index":0,"final":true,
                "delta":"I'll run the bash command and spawn the agent."}"#,
        )
        .expect("the real MessageDisplay payload must deserialize");

        assert_eq!(
            input.message.as_deref(),
            Some("I'll run the bash command and spawn the agent."),
            "`delta` must reach the message field — this is the whole bug"
        );
        assert_eq!(input.message_is_final, Some(true));
        assert_eq!(input.index, Some(0));
        assert_eq!(
            input.display_text(),
            Some("I'll run the bash command and spawn the agent.")
        );
    }

    /// A non-final chunk is not transformable: a fence or a secret split across
    /// a streaming boundary cannot be judged from one chunk.
    #[test]
    fn display_text_declines_a_non_final_chunk() {
        let partial: HookInput = serde_json::from_str(
            r#"{"hook_event_name":"MessageDisplay","index":0,"final":false,
                "delta":"here is a fenced bl"}"#,
        )
        .unwrap();
        assert_eq!(partial.message.as_deref(), Some("here is a fenced bl"));
        assert_eq!(
            partial.display_text(),
            None,
            "a partial chunk must not be transformed"
        );

        // A harness that omits `final` is treated as final, not silently skipped.
        let unmarked: HookInput =
            serde_json::from_str(r#"{"hook_event_name":"MessageDisplay","delta":"whole text"}"#)
                .unwrap();
        assert_eq!(unmarked.display_text(), Some("whole text"));

        // Blank is None, same rule as `submitted_prompt`.
        let blank: HookInput =
            serde_json::from_str(r#"{"hook_event_name":"MessageDisplay","delta":"  \n "}"#)
                .unwrap();
        assert_eq!(blank.display_text(), None);
    }

    /// cas-5e46: raw interactive `idle_prompt` capture confirms that Notification
    /// really does use `message`, so aliasing `delta` onto the same field must
    /// not have broken it. `notification_type` is intentionally undeclared: no
    /// current CAS handler consumes it.
    #[test]
    fn notification_idle_prompt_payload_uses_the_message_key() {
        let input: HookInput = serde_json::from_str(
            r#"{"session_id":"d59d0086-c4cf-4a81-94ef-30ca97b8490e",
                "transcript_path":"/tmp/wirecap/projects/-home-pippenz-Petrastella-cas-src--cas-worktrees-warm-newt-60/d59d0086-c4cf-4a81-94ef-30ca97b8490e.jsonl",
                "cwd":"/home/pippenz/Petrastella/cas-src/.cas/worktrees/warm-newt-60",
                "prompt_id":"3b983dd9-56b7-4672-b40a-161b8cad74f1",
                "hook_event_name":"Notification",
                "message":"Claude is waiting for your input",
                "notification_type":"idle_prompt"}"#,
        )
        .expect("the real Notification payload must deserialize");
        assert_eq!(
            input.message.as_deref(),
            Some("Claude is waiting for your input")
        );
    }

    /// Stop carries `stop_hook_active`, the harness's loop-prevention signal.
    /// It was declared nowhere while CAS blocked Stop in five places.
    #[test]
    fn stop_payload_carries_stop_hook_active() {
        let quiet: HookInput = serde_json::from_str(
            r#"{"session_id":"fc70e6f3","transcript_path":"/tmp/x.jsonl","cwd":"/tmp/wirecap/proj",
                "prompt_id":"d323a56b","permission_mode":"default","hook_event_name":"Stop",
                "stop_hook_active":false,"last_assistant_message":"DONE",
                "background_tasks":[{"id":"a24c4ba99866d62e3","type":"subagent",
                    "status":"running","description":"Reply with pong",
                    "agent_type":"general-purpose"}],
                "session_crons":[]}"#,
        )
        .expect("the real Stop payload must deserialize");
        assert_eq!(quiet.stop_hook_active, Some(false));
        assert!(!quiet.stop_hook_is_reentrant());

        let reentrant: HookInput =
            serde_json::from_str(r#"{"hook_event_name":"Stop","stop_hook_active":true}"#).unwrap();
        assert!(
            reentrant.stop_hook_is_reentrant(),
            "true must mean `already continuing because a stop hook blocked`"
        );

        // Absent means not re-entrant, so harnesses that omit the key are
        // unchanged.
        let absent: HookInput = serde_json::from_str(r#"{"hook_event_name":"Stop"}"#).unwrap();
        assert!(!absent.stop_hook_is_reentrant());
    }

    /// SubagentStop carries `stop_hook_active` too, plus the child identity
    /// under `agent_id` / `agent_type`.
    #[test]
    fn subagentstop_payload_carries_stop_hook_active_and_agent_identity() {
        let input: HookInput = serde_json::from_str(
            r#"{"session_id":"fc70e6f3","transcript_path":"/tmp/x.jsonl","cwd":"/tmp/wirecap/proj",
                "prompt_id":"d323a56b","permission_mode":"default",
                "agent_id":"a24c4ba99866d62e3","agent_type":"general-purpose",
                "hook_event_name":"SubagentStop","stop_hook_active":false,
                "agent_transcript_path":"/tmp/x/subagents/agent-a24c4ba99866d62e3.jsonl",
                "last_assistant_message":"pong","background_tasks":[],"session_crons":[]}"#,
        )
        .expect("the real SubagentStop payload must deserialize");
        assert_eq!(input.stop_hook_active, Some(false));
        assert_eq!(input.agent_id.as_deref(), Some("a24c4ba99866d62e3"));
        assert_eq!(input.agent_type.as_deref(), Some("general-purpose"));
    }

    /// cas-f3e3 Finding 3, resolved against the wire: the child's type arrives
    /// as `agent_type`. Nothing named `subagent_type` / `subagentType` is sent,
    /// so that field is dead on arrival and must not gate behaviour. Same for
    /// `subagent_prompt` — SubagentStart sends no prompt at all, which is the
    /// independent confirmation that cas-78d3's alias move cost nothing.
    #[test]
    fn subagentstart_payload_uses_agent_type_not_subagent_type() {
        let input: HookInput = serde_json::from_str(
            r#"{"session_id":"fc70e6f3","transcript_path":"/tmp/x.jsonl","cwd":"/tmp/wirecap/proj",
                "prompt_id":"d323a56b","agent_id":"a24c4ba99866d62e3",
                "agent_type":"general-purpose","hook_event_name":"SubagentStart"}"#,
        )
        .expect("the real SubagentStart payload must deserialize");

        assert_eq!(input.agent_type.as_deref(), Some("general-purpose"));
        assert_eq!(input.agent_id.as_deref(), Some("a24c4ba99866d62e3"));
        assert_eq!(
            input.subagent_type, None,
            "no wire key populates subagent_type — do not read it"
        );
        assert_eq!(
            input.subagent_prompt, None,
            "SubagentStart sends no prompt field"
        );
    }

    /// The common envelope every event shares, captured rather than assumed —
    /// including `prompt_id`, which CAS does not declare. Unknown keys must
    /// stay ignorable so an added harness field can never make a hook fail.
    #[test]
    fn captured_envelopes_deserialize_with_undeclared_keys_ignored() {
        let cases = [
            // SessionStart
            r#"{"session_id":"fc70e6f3","transcript_path":"/tmp/x.jsonl","cwd":"/tmp/wirecap/proj",
                "hook_event_name":"SessionStart","source":"startup"}"#,
            // SessionEnd
            r#"{"session_id":"fc70e6f3","transcript_path":"/tmp/x.jsonl","cwd":"/tmp/wirecap/proj",
                "prompt_id":"4084091b","hook_event_name":"SessionEnd","reason":"other"}"#,
            // PostToolUseFailure (undeclared: error, is_interrupt, duration_ms)
            r#"{"session_id":"e0dde9dc","transcript_path":"/tmp/x.jsonl","cwd":"/tmp/wirecap/proj",
                "prompt_id":"cb998ec6","permission_mode":"default",
                "hook_event_name":"PostToolUseFailure","tool_name":"Bash",
                "tool_input":{"command":"exit 7","description":"Exit with code 7"},
                "tool_use_id":"toolu_01EmGh","error":"Exit code 7",
                "is_interrupt":false,"duration_ms":310}"#,
        ];
        for body in cases {
            let input: HookInput =
                serde_json::from_str(body).unwrap_or_else(|e| panic!("{body}\n->{e}"));
            assert_eq!(input.session_id.is_empty(), false, "{body}");
            assert!(!input.hook_event_name.is_empty(), "{body}");
            assert_eq!(input.cwd, "/tmp/wirecap/proj", "{body}");
        }
    }

    /// The captured PostToolUse payload, including the real `tool_response`
    /// shape and the undeclared `duration_ms`.
    #[test]
    fn posttooluse_payload_lands_in_the_declared_fields() {
        let input: HookInput = serde_json::from_str(
            r#"{"session_id":"fc70e6f3","transcript_path":"/tmp/x.jsonl","cwd":"/tmp/wirecap/proj",
                "prompt_id":"d323a56b","permission_mode":"default",
                "hook_event_name":"PostToolUse","tool_name":"Bash",
                "tool_input":{"command":"echo wirecap-one","description":"Echo wirecap-one"},
                "tool_response":{"stdout":"wirecap-one","stderr":"","interrupted":false,
                    "isImage":false,"noOutputExpected":false},
                "tool_use_id":"toolu_019d9F7UQXCNd1ajCGTtJbbL","duration_ms":277}"#,
        )
        .expect("the real PostToolUse payload must deserialize");

        assert_eq!(input.tool_name.as_deref(), Some("Bash"));
        assert_eq!(input.tool_use_id.as_deref(), Some("toolu_019d9F7UQXCNd1ajCGTtJbbL"));
        assert_eq!(input.permission_mode.as_deref(), Some("default"));
        assert_eq!(
            input
                .tool_response
                .as_ref()
                .and_then(|v| v.get("stdout"))
                .and_then(|v| v.as_str()),
            Some("wirecap-one"),
        );
    }

    #[test]
    fn pretooluse_serializes_with_event_tag() {
        // The #[serde(tag = "hookEventName")] directive must produce the same
        // wire shape the old flat-struct code emitted: hookEventName as a
        // sibling key inside the hookSpecificOutput object, alongside the
        // permission fields.
        let out = HookOutput::with_pre_tool_permission("allow", "ok");
        let json = serde_json::to_string(&out).unwrap();
        assert_eq!(
            json,
            r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","permissionDecisionReason":"ok"}}"#,
            "PreToolUse wire shape regressed: {json}"
        );
    }

    #[test]
    fn userpromptsubmit_serializes_with_event_tag() {
        let out = HookOutput::with_user_prompt_context("ctx".into());
        let json = serde_json::to_string(&out).unwrap();
        assert_eq!(
            json,
            r#"{"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":"ctx"}}"#,
            "UserPromptSubmit wire shape regressed: {json}"
        );
    }

    #[test]
    fn posttooluse_serializes_with_event_tag() {
        // PostToolUse's additionalContext is Option — when present it emits
        // the field, when None it must be ABSENT (not `"additionalContext":null`)
        // per `skip_serializing_if = "Option::is_none"`.
        let with_ctx = HookOutput::with_post_tool_context("ripple reminder".into());
        let json = serde_json::to_string(&with_ctx).unwrap();
        assert_eq!(
            json,
            r#"{"hookSpecificOutput":{"hookEventName":"PostToolUse","additionalContext":"ripple reminder"}}"#,
            "PostToolUse wire shape regressed: {json}"
        );
    }

    #[test]
    fn sessionstart_serializes_with_event_tag() {
        let out = HookOutput::with_session_start_context("CAS active: 3 tasks".into());
        let json = serde_json::to_string(&out).unwrap();
        assert_eq!(
            json,
            r#"{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"CAS active: 3 tasks"}}"#,
            "SessionStart wire shape regressed: {json}"
        );
    }

    #[test]
    fn pretooluse_skips_missing_optional_fields() {
        // Guard against serde enum-tagging regression: fields with
        // skip_serializing_if must still be omitted (not null) inside a
        // tagged-enum variant. Old flat-struct code had this behavior;
        // regressing would introduce stray null keys that validators reject.
        let with_input = HookOutput::with_pre_tool_updated_input(serde_json::json!({"x": 1}));
        let json = serde_json::to_string(&with_input).unwrap();
        assert!(
            !json.contains("null"),
            "PreToolUse updated-input output must not serialize any null-valued key: {json}"
        );
        assert!(
            !json.contains("permissionDecision"),
            "with_pre_tool_updated_input must not emit permissionDecision key: {json}"
        );
    }

    #[test]
    fn permissionrequest_serializes_with_event_tag() {
        // PermissionRequest's permissionDecision lives INSIDE hookSpecificOutput
        // (per Claude Code's PermissionRequest event surface), not at the top
        // level. The top-level `permissionDecision` field on HookOutput is for
        // separate event surfaces and is not set here. Confirms no shadow.
        let out = HookOutput::with_permission_request("deny", "blocked");
        let json = serde_json::to_string(&out).unwrap();
        assert_eq!(
            json,
            r#"{"hookSpecificOutput":{"hookEventName":"PermissionRequest","permissionDecision":"deny","permissionDecisionReason":"blocked"}}"#,
            "PermissionRequest wire shape regressed: {json}"
        );
    }
}
