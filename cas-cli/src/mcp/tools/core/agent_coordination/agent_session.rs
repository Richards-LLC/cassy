use crate::mcp::tools::core::imports::*;

fn public_registration_hints(
    agent_type: Option<&str>,
) -> Result<
    (
        Option<crate::types::AgentType>,
        Option<crate::types::AgentRole>,
    ),
    McpError,
> {
    let requested_role = agent_type.and_then(|value| value.parse().ok());
    if matches!(
        requested_role,
        Some(crate::types::AgentRole::Supervisor | crate::types::AgentRole::Director)
    ) {
        return Err(McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(
                "Public agent registration cannot request supervisor or director authority.",
            ),
            data: None,
        });
    }
    let requested_type = agent_type.and_then(|value| value.parse().ok());
    let safe_role = (requested_type == Some(crate::types::AgentType::Worker))
        .then_some(crate::types::AgentRole::Worker);
    Ok((requested_type, safe_role))
}

impl CasCore {
    pub async fn cas_agent_register(
        &self,
        Parameters(req): Parameters<AgentRegisterRequest>,
    ) -> Result<CallToolResult, McpError> {
        // Session ID is required - it becomes the agent's unique identifier
        let session_id = req.session_id.ok_or_else(|| McpError {
            code: ErrorCode::INVALID_PARAMS,
            message: Cow::from(
                "session_id is required. Use the session ID from your SessionStart context.",
            ),
            data: None,
        })?;

        let (requested_agent_type, requested_role) =
            public_registration_hints(Some(&req.agent_type))?;
        let agent_name = req.name.clone();

        // Use explicit type/role hints when provided.
        let id = self.register_agent_with_hints(
            session_id,
            req.name,
            req.parent_id,
            requested_agent_type,
            requested_role,
            crate::mcp::server::AgentIdentitySource::PublicRegistration,
        )?;

        // cas-6913: surface any prompt_queue messages that were queued to
        // this agent's name before it existed in the agent store — see
        // `pending_mail_for_registration` for the full rationale.
        let pending_mail = self.pending_mail_for_registration(&agent_name);

        Ok(Self::success(format!(
            "Registered agent: {id}{pending_mail}"
        )))
    }

    /// Start a session without Claude hooks (Codex-friendly)
    pub async fn cas_agent_session_start(
        &self,
        Parameters(req): Parameters<SessionStartRequest>,
    ) -> Result<CallToolResult, McpError> {
        let agent_name = req
            .name
            .clone()
            .or_else(|| std::env::var("CAS_AGENT_NAME").ok())
            .unwrap_or_else(|| "Codex".to_string());
        let session_id = req.session_id.unwrap_or_else(|| {
            let name = agent_name.to_lowercase();
            let safe_name: String = name
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() {
                        c.to_ascii_lowercase()
                    } else {
                        '-'
                    }
                })
                .collect();
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            format!("codex-{}-{}", safe_name.trim_matches('-'), ts)
        });

        let (requested_agent_type, requested_role) =
            public_registration_hints(req.agent_type.as_deref())?;
        self.ensure_public_registration_target(&session_id)?;

        // Best-effort name hint for session registration
        if let Some(ref name) = req.name {
            unsafe { std::env::set_var("CAS_AGENT_NAME", name) };
        } else if std::env::var("CAS_AGENT_NAME").is_err() {
            unsafe { std::env::set_var("CAS_AGENT_NAME", &agent_name) };
        }
        let cwd = req.cwd.unwrap_or_else(|| {
            self.cas_root
                .parent()
                .unwrap_or(&self.cas_root)
                .to_string_lossy()
                .to_string()
        });

        let input = HookInput {
            session_id: session_id.clone(),
            cwd,
            workspace_root: None,
            hook_event_name: "SessionStart".to_string(),
            transcript_path: None,
            permission_mode: req.permission_mode,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            tool_use_id: None,
            tool_input_truncated: None,
            user_prompt: None,
            source: Some("codex".to_string()),
            reason: None,
            subagent_type: None,
            agent_id: None,
            agent_type: None,
            subagent_prompt: None,
            // Carry the role explicitly so downstream handlers don't depend on
            // the process-global env mutation above (which races under shared
            // MCP process dispatch).
            agent_role: requested_role.map(|r| r.to_string()),
            message: None,
            message_is_final: None,
            index: None,
            stop_hook_active: None,
        };

        // Use hook handler for session start side effects
        let output = handle_session_start(&input, Some(&self.cas_root)).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to start session: {e}")),
            data: None,
        })?;

        // SessionStart is the only variant handle_session_start can legitimately
        // produce on its context-injection path. Match exhaustively so the
        // compiler forces a decision if a new variant is ever added — a
        // wildcard `_` arm here would silently drop context on a future
        // mis-wire, which is exactly the class of bug cas-e55b was meant to
        // eliminate.
        use crate::hooks::HookSpecificOutput;
        let context = match output.hook_specific_output {
            Some(HookSpecificOutput::SessionStart {
                additional_context, ..
            }) => Some(additional_context),
            // None variants below are unreachable in practice (handle_session_start
            // never emits these shapes), but pattern-matching them explicitly
            // makes the invariant load-bearing on the type system rather than on
            // a comment.
            Some(HookSpecificOutput::PreToolUse { .. })
            | Some(HookSpecificOutput::UserPromptSubmit { .. })
            | Some(HookSpecificOutput::PostToolUse { .. })
            | Some(HookSpecificOutput::PermissionRequest { .. })
            | Some(HookSpecificOutput::MessageDisplay { .. })
            | None => None,
        }
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| {
            build_context(&input, req.limit.unwrap_or(5), &self.cas_root)
                .unwrap_or_else(|_| String::new())
        });

        // Write current_session for CLI parity (best effort)
        let _ = std::fs::write(self.cas_root.join("current_session"), &session_id);

        // Ensure the agent is registered immediately for subsequent MCP calls (whoami/task/...).
        // In Codex no-hooks mode, relying on PID/session mapping can race.
        self.register_agent_with_hints(
            session_id.clone(),
            agent_name.clone(),
            req.parent_id,
            requested_agent_type,
            requested_role,
            crate::mcp::server::AgentIdentitySource::PublicRegistration,
        )?;
        let _ = self.ensure_agent_active(&session_id);

        let mut response = format!("Session: {session_id}");
        if !context.is_empty() {
            response.push_str("\n\n");
            response.push_str(&context);
        }

        // cas-6913: surface any prompt_queue messages queued to this agent's
        // name before it existed in the agent store (the "first-claim
        // stall" gap — see `pending_mail_for_registration`). This is the
        // codex-worker session_start path, the exact repro in the source
        // bug doc (BUG-worker-first-claim-stall-2026-07-07.md).
        response.push_str(&self.pending_mail_for_registration(&agent_name));

        Ok(Self::success(response))
    }

    /// cas-6913 AC1 (deliver-on-register): render any `prompt_queue`
    /// messages already addressed to `agent_name` (or `all_workers`) as
    /// text to append to the registration response.
    ///
    /// This closes the "first-claim stall" gap at its root: registration
    /// (`cas_agent_register` / `cas_agent_session_start`) previously had
    /// zero wiring to `prompt_queue` at all, so a message queued to a
    /// not-yet-registered worker name relied entirely on the daemon's
    /// PTY-injection poll noticing the name later — a poll whose target
    /// list is built from daemon-side bookkeeping that can lag behind
    /// what the supervisor already knows (see task notes for the traced
    /// mechanism). Returning the message text directly in the tool result
    /// the agent's own registration call receives delivers it into the
    /// agent's context immediately, with no PTY timing dependency.
    ///
    /// A direct message rendered here is consumed by the recipient through the
    /// registration tool result itself. Mark it transport-delivered and acked
    /// before returning so the daemon cannot inject the same row as a second
    /// fresh turn. `all_workers` is left to the daemon because one registering
    /// worker cannot confirm a multi-recipient broadcast on everyone else's
    /// behalf. Best-effort: any store error degrades to an empty string rather
    /// than failing registration itself.
    fn pending_mail_for_registration(&self, agent_name: &str) -> String {
        use crate::store::open_prompt_queue_store;

        if agent_name.is_empty() {
            return String::new();
        }

        let Ok(queue) = open_prompt_queue_store(&self.cas_root) else {
            return String::new();
        };
        let factory_session = std::env::var("CAS_FACTORY_SESSION")
            .ok()
            .filter(|s| !s.trim().is_empty());
        let Ok(pending) =
            queue.peek_for_targets(&[agent_name, "all_workers"], factory_session.as_deref(), 20)
        else {
            return String::new();
        };

        if pending.is_empty() {
            return String::new();
        }

        let mut out = format!(
            "\n\n📬 {} message(s) were already waiting for you (queued before you registered):",
            pending.len()
        );
        for msg in &pending {
            let summary = msg
                .summary
                .as_deref()
                .map(|s| format!(" — {s}"))
                .unwrap_or_default();
            out.push_str(&format!(
                "\n\n**From {}**{summary}\n{}",
                msg.source, msg.prompt
            ));
            if msg.target.eq_ignore_ascii_case(agent_name)
                && queue.mark_transport_delivered(msg.id).is_ok()
            {
                let _ = queue.ack(msg.id);
            }
        }
        out
    }

    /// End a session without Claude hooks (Codex-friendly)
    pub async fn cas_agent_session_end(
        &self,
        Parameters(req): Parameters<SessionEndRequest>,
    ) -> Result<CallToolResult, McpError> {
        let session_id = match req.session_id {
            Some(id) => id,
            None => self.get_agent_id()?,
        };

        let cwd = self
            .cas_root
            .parent()
            .unwrap_or(&self.cas_root)
            .to_string_lossy()
            .to_string();

        let input = HookInput {
            session_id: session_id.clone(),
            cwd,
            workspace_root: None,
            hook_event_name: "SessionEnd".to_string(),
            transcript_path: None,
            permission_mode: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            tool_use_id: None,
            tool_input_truncated: None,
            user_prompt: None,
            source: Some("codex".to_string()),
            reason: req.reason,
            subagent_type: None,
            agent_id: None,
            agent_type: None,
            subagent_prompt: None,
            agent_role: std::env::var("CAS_AGENT_ROLE").ok(),
            message: None,
            message_is_final: None,
            index: None,
            stop_hook_active: None,
        };

        handle_session_end(&input, Some(&self.cas_root)).map_err(|e| McpError {
            code: ErrorCode::INTERNAL_ERROR,
            message: Cow::from(format!("Failed to end session: {e}")),
            data: None,
        })?;

        let _ = std::fs::remove_file(self.cas_root.join("current_session"));

        let store = self.open_agent_store()?;
        let _ = store.unregister(&session_id);

        Ok(Self::success(format!("Session ended: {session_id}")))
    }
}
