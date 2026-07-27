use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Supported interactive harnesses for factory panes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SupervisorCli {
    Claude,
    Codex,
    /// xAI Grok Build (grok 0.2.93+). Namespaces MCP tools as
    /// `<server>__<tool>` (e.g. `cas__task`) via its own search_tool/
    /// use_tool dispatch — NOT `mcp__cas__` (Claude) or `mcp__cs__`
    /// (Codex). Maps to the Claude capability tier (hooks + subagents +
    /// textbox submit all work), but coordinates like Codex (no CC
    /// agent-teams --team-name/--agent-id; MCP + prompt injection only).
    /// See EPIC cas-8888.
    Grok,
}

impl SupervisorCli {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Grok => "grok",
        }
    }

    pub fn capabilities(self) -> HarnessCapabilities {
        match self {
            Self::Claude => HarnessCapabilities {
                supports_hooks: true,
                supports_subagents: true,
                supports_textbox_submit: true,
                tool_prefix: "mcp__cas__",
            },
            Self::Codex => HarnessCapabilities {
                supports_hooks: false,
                supports_subagents: false,
                supports_textbox_submit: false,
                tool_prefix: "mcp__cs__",
            },
            Self::Grok => HarnessCapabilities {
                supports_hooks: true,
                supports_subagents: true,
                supports_textbox_submit: true,
                tool_prefix: "cas__",
            },
        }
    }

    /// Bytes that cancel the current in-flight turn for this harness.
    ///
    /// Used by factory turn-break (`Pane::break_turn`, Escape routing, and the
    /// urgent interrupt-and-redirect path) so Stop / Esc / programmatic cancel
    /// share one harness-aware payload (cas-7f6f):
    ///
    /// - **Claude / Codex**: Esc (`0x1b`) — Claude Code's cancel-turn key.
    /// - **Grok**: Ctrl+C (`0x03`) — since 0.2.93 Esc is a mid-turn no-op and
    ///   cancel is Ctrl+C (empty prompt; non-empty draft clears first).
    pub fn turn_cancel_bytes(self) -> &'static [u8] {
        match self {
            Self::Claude | Self::Codex => &[0x1b],
            Self::Grok => &[0x03],
        }
    }
}

impl FromStr for SupervisorCli {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "claude" => Ok(Self::Claude),
            "codex" => Ok(Self::Codex),
            "grok" => Ok(Self::Grok),
            _ => Err(format!("unsupported harness: {s}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HarnessCapabilities {
    pub supports_hooks: bool,
    pub supports_subagents: bool,
    /// Whether this harness's turn-cancel (Esc) returns to an input-ready
    /// state promptly enough that a flat post-cancel sleep is safe before
    /// injecting a follow-up prompt. Consumed by
    /// `Mux::wait_for_injection_readiness` (cas-4208): `true` (Claude, Grok)
    /// keeps the old flat-sleep-only settle verbatim; `false` (Codex) — whose
    /// TUI renders a transitional "Conversation interrupted" banner after Esc
    /// that a flat sleep can silently race, swallowing the follow-up submit —
    /// gets an active `pane_bytes_received` quiescence poll instead of a
    /// guessed constant.
    pub supports_textbox_submit: bool,
    pub tool_prefix: &'static str,
}

#[cfg(test)]
mod tests {
    //! cas-9a31 (EPIC cas-8888, Phase 1): Grok harness-core coverage.
    use super::*;

    #[test]
    fn grok_as_str_and_from_str_round_trip() {
        assert_eq!(SupervisorCli::Grok.as_str(), "grok");
        assert_eq!(SupervisorCli::from_str("grok"), Ok(SupervisorCli::Grok));
        // Case/whitespace tolerance, matching Claude/Codex's existing contract.
        assert_eq!(SupervisorCli::from_str("Grok"), Ok(SupervisorCli::Grok));
        assert_eq!(SupervisorCli::from_str("  grok  "), Ok(SupervisorCli::Grok));
    }

    #[test]
    fn grok_capabilities_match_claude_tier_with_its_own_tool_prefix() {
        let caps = SupervisorCli::Grok.capabilities();
        assert!(
            caps.supports_hooks,
            "Grok's SessionStart/PreToolUse/PostToolUse/Stop hooks are fully wired \
             (verified live per EPIC cas-8888)"
        );
        assert!(
            caps.supports_subagents,
            "Grok supports the same subagent model as Claude"
        );
        assert!(
            caps.supports_textbox_submit,
            "Grok supports textbox-submit interaction like Claude"
        );
        assert_eq!(
            caps.tool_prefix, "cas__",
            "Grok namespaces MCP tools as <server>__<tool> (cas__task), \
             distinct from Claude's mcp__cas__ and Codex's mcp__cs__"
        );
    }

    #[test]
    fn grok_serde_round_trips_as_lowercase_grok() {
        let json = serde_json::to_string(&SupervisorCli::Grok).unwrap();
        assert_eq!(json, "\"grok\"");
        let back: SupervisorCli = serde_json::from_str(&json).unwrap();
        assert_eq!(back, SupervisorCli::Grok);
    }

    #[test]
    fn from_str_rejects_unknown_harness() {
        assert!(SupervisorCli::from_str("gemini").is_err());
    }

    #[test]
    fn claude_and_codex_capabilities_unchanged_by_the_grok_addition() {
        // Regression pin: adding Grok must not perturb the existing two.
        let claude = SupervisorCli::Claude.capabilities();
        assert!(claude.supports_hooks && claude.supports_subagents && claude.supports_textbox_submit);
        assert_eq!(claude.tool_prefix, "mcp__cas__");

        let codex = SupervisorCli::Codex.capabilities();
        assert!(!codex.supports_hooks && !codex.supports_subagents && !codex.supports_textbox_submit);
        assert_eq!(codex.tool_prefix, "mcp__cs__");
    }

    /// cas-7f6f: Grok cancels with Ctrl+C; Claude/Codex keep Esc.
    #[test]
    fn turn_cancel_bytes_are_harness_aware() {
        assert_eq!(SupervisorCli::Claude.turn_cancel_bytes(), &[0x1b]);
        assert_eq!(SupervisorCli::Codex.turn_cancel_bytes(), &[0x1b]);
        assert_eq!(SupervisorCli::Grok.turn_cancel_bytes(), &[0x03]);
    }
}
