//! Commander exposes semantic supervisor commands, never caller-supplied PTY text.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorCommand {
    Compact,
    Clear,
    Model,
    Usage,
    Exit,
    Finish,
}

impl SupervisorCommand {
    pub fn needs_confirmation(self) -> bool {
        matches!(self, Self::Clear | Self::Exit | Self::Finish)
    }

    /// Resolves from the daemon-owned pane's harness, never a client hint.
    pub fn mapping(self, harness: cas_mux::SupervisorCli) -> Option<&'static str> {
        use cas_mux::SupervisorCli::{Claude, Codex};
        match (harness, self) {
            (Claude | Codex, Self::Compact) => Some("/compact"),
            (Claude | Codex, Self::Clear) => Some("/clear"),
            (Claude | Codex, Self::Model) => Some("/model"),
            (Claude, Self::Usage) => Some("/usage"),
            (Codex, Self::Usage) => Some("/status"),
            (Claude | Codex, Self::Exit) => Some("/exit"),
            (Claude, Self::Finish) => Some("/finish"),
            (Codex, Self::Finish) => Some("$finish"),
            _ => None,
        }
    }
}

/// Fixed navigation for a native command's own chooser, never a general keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorCommandKey { Previous, Next, Select, Cancel }

impl SupervisorCommandKey {
    pub fn bytes(self) -> &'static [u8] {
        match self {
            Self::Previous => b"\x1b[A",
            Self::Next => b"\x1b[B",
            Self::Select => b"\r",
            Self::Cancel => b"\x1b",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorCommandPhase { Submitted, Observed, Refused, Unconfirmed }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisorCommandReceipt {
    pub client_ref: String,
    pub command: SupervisorCommand,
    pub phase: SupervisorCommandPhase,
    pub detail: String,
    /// Native harness output, redacted and bounded. A chooser is not a model change.
    pub output: String,
    pub interactive: bool,
}
