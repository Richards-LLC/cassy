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
                requires_bracketed_paste_injection: false,
                tool_prefix: "mcp__cas__",
            },
            Self::Codex => HarnessCapabilities {
                supports_hooks: false,
                supports_subagents: false,
                supports_textbox_submit: false,
                requires_bracketed_paste_injection: true,
                tool_prefix: "mcp__cs__",
            },
            Self::Grok => HarnessCapabilities {
                supports_hooks: true,
                supports_subagents: true,
                supports_textbox_submit: true,
                requires_bracketed_paste_injection: false,
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
    /// Whether PTY prompt injection must wrap the payload in explicit
    /// bracketed-paste delimiters (`ESC[200~` … `ESC[201~`) before the
    /// trailing CR that submits it (cas-5fff).
    ///
    /// Codex's TUI runs a **paste-burst detector** over unframed input: a
    /// single large write is classified as a paste that is still arriving, so
    /// the CR that follows is consumed as the paste's terminator (inserted as
    /// a newline) instead of being read as an Enter keypress. The payload is
    /// left in the composer as an unsubmitted draft and every later message
    /// types more text into that same stuck draft — the exact "delivered but
    /// no turn ever starts" signature reported in cas-5fff.
    ///
    /// Live-measured against `codex` 0.146.0 through this crate's own
    /// `Mux`/`Pane` code (`tests/nonurgent_idle_codex_runtime.rs`):
    /// - ~78-byte single-line payload + CR → submits.
    /// - 1045-byte payload + CR → swallowed, draft left in the composer.
    /// - Same 1045 bytes with every newline replaced by a space → **also**
    ///   swallowed, so the trigger is burst SIZE, not newlines.
    /// - Sending the CR at confirmed output quiescence (measured at +375ms,
    ///   i.e. *earlier* than the old blind 500ms settle) → still swallowed,
    ///   so this is NOT a settle-timing race and cannot be tuned away.
    /// - Same payload wrapped in `ESC[200~`…`ESC[201~` + CR → submits, and
    ///   the model replies.
    ///
    /// Declaring the paste's end removes the ambiguity, which is why this is a
    /// deterministic fix rather than a longer guess. `false` for Claude and
    /// Grok: both have a real textbox submit and were never implicated (their
    /// injection bytes stay byte-for-byte unchanged).
    pub requires_bracketed_paste_injection: bool,
    pub tool_prefix: &'static str,
}

/// Bracketed-paste start sequence (`ESC[200~`).
pub const PASTE_START: &[u8] = b"\x1b[200~";
/// Bracketed-paste end sequence (`ESC[201~`).
pub const PASTE_END: &[u8] = b"\x1b[201~";

/// Build the exact byte payload written to a PTY for a prompt injection,
/// excluding the trailing submit CR (cas-5fff).
///
/// Pure and exhaustively unit-testable so the framing contract can't drift
/// between the normal and urgent injection paths, both of which funnel
/// through `Pane::inject_prompt`.
pub fn injection_payload_bytes(harness: SupervisorCli, text: &str) -> Vec<u8> {
    if harness.capabilities().requires_bracketed_paste_injection {
        let mut out = Vec::with_capacity(text.len() + PASTE_START.len() + PASTE_END.len());
        out.extend_from_slice(PASTE_START);
        out.extend_from_slice(text.as_bytes());
        out.extend_from_slice(PASTE_END);
        out
    } else {
        text.as_bytes().to_vec()
    }
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

    /// cas-5fff: only Codex needs bracketed-paste framing. Pinned explicitly
    /// because getting this wrong is silent — an unframed Codex injection
    /// still "succeeds" at the PTY layer and simply never starts a turn.
    #[test]
    fn only_codex_requires_bracketed_paste_injection() {
        assert!(
            SupervisorCli::Codex
                .capabilities()
                .requires_bracketed_paste_injection
        );
        assert!(
            !SupervisorCli::Claude
                .capabilities()
                .requires_bracketed_paste_injection
        );
        assert!(
            !SupervisorCli::Grok
                .capabilities()
                .requires_bracketed_paste_injection
        );
    }

    /// cas-5fff: Codex payloads are wrapped; Claude/Grok stay byte-for-byte
    /// identical to the pre-fix bytes (AC5 — no behavior change off Codex).
    #[test]
    fn injection_payload_is_bracketed_only_for_codex() {
        let text = "Message from supervisor: re-close cas-ae2f\n\nSecond paragraph.";

        let codex = injection_payload_bytes(SupervisorCli::Codex, text);
        assert!(codex.starts_with(PASTE_START), "codex payload must open the paste");
        assert!(codex.ends_with(PASTE_END), "codex payload must close the paste");
        assert_eq!(
            &codex[PASTE_START.len()..codex.len() - PASTE_END.len()],
            text.as_bytes(),
            "the payload between the delimiters must be untouched"
        );

        for bare in [SupervisorCli::Claude, SupervisorCli::Grok] {
            assert_eq!(
                injection_payload_bytes(bare, text),
                text.as_bytes(),
                "{bare:?} injection bytes must be unchanged by the cas-5fff fix"
            );
        }
    }

    /// The delimiters must be the real terminal sequences — a typo here would
    /// be injected verbatim into the worker's prompt and, worse, would restore
    /// the original swallow.
    #[test]
    fn bracketed_paste_delimiters_are_the_standard_sequences() {
        assert_eq!(PASTE_START, b"\x1b[200~");
        assert_eq!(PASTE_END, b"\x1b[201~");
    }

    /// Empty payloads must not gain delimiters-only noise beyond the framing
    /// itself, and must not panic on the slice arithmetic above.
    #[test]
    fn injection_payload_handles_empty_text() {
        let codex = injection_payload_bytes(SupervisorCli::Codex, "");
        assert_eq!(codex.len(), PASTE_START.len() + PASTE_END.len());
        assert!(injection_payload_bytes(SupervisorCli::Claude, "").is_empty());
    }

    /// cas-7f6f: Grok cancels with Ctrl+C; Claude/Codex keep Esc.
    #[test]
    fn turn_cancel_bytes_are_harness_aware() {
        assert_eq!(SupervisorCli::Claude.turn_cancel_bytes(), &[0x1b]);
        assert_eq!(SupervisorCli::Codex.turn_cancel_bytes(), &[0x1b]);
        assert_eq!(SupervisorCli::Grok.turn_cancel_bytes(), &[0x03]);
    }
}
