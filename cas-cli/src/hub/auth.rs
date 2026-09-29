//! Protocol-specific policy remains in cas; persistence lives in the spike crate.

pub use cas_hub_state::auth::*;
use crate::ui::factory::ClientMessage;

pub fn required_scope(message: &ClientMessage) -> Option<Scope> {
    match message {
        ClientMessage::Input { .. }
        | ClientMessage::InputFocused { .. }
        | ClientMessage::Focus { .. }
        | ClientMessage::FocusNext
        | ClientMessage::FocusPrev
        | ClientMessage::Resize { .. } => Some(Scope::PaneInput),
        // Reporting the viewport is part of observing a pane, not terminal
        // input. The lease policy is enforced separately: an unleased pane
        // may follow an observer, while a leased pane follows its controller.
        ClientMessage::ResizePane { .. }
        | ClientMessage::RequestPaneKeyframe { .. }
        | ClientMessage::ScrollbackRequest { .. }
        | ClientMessage::ConversationHistoryRequest { .. } => Some(Scope::PaneRead),
        ClientMessage::SendMessage { .. } => Some(Scope::MessageSend),
        ClientMessage::InterruptPane { .. } => Some(Scope::PaneInterrupt),
        ClientMessage::SpawnWorkers { .. }
        | ClientMessage::ShutdownWorkers { .. }
        | ClientMessage::Inject { .. }
        | ClientMessage::SpawnShell { .. }
        | ClientMessage::KillShell { .. } => Some(Scope::FactoryManage),
        // The legacy focused-pane interrupt is intentionally never exposed.
        ClientMessage::Interrupt => None,
        ClientMessage::Attach { .. }
        | ClientMessage::Detach
        | ClientMessage::GetState
        | ClientMessage::Ping
        | ClientMessage::OperatorReplyDelivered { .. } => None,
    }
}
