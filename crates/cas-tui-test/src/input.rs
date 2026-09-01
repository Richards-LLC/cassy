//! Input DSL - Builder for composing input sequences
//!
//! Provides an ergonomic way to build sequences of terminal input
//! including text, key presses, and delays.

use crate::pty_runner::{Key, PtyRunner, PtyRunnerError};
use std::time::Duration;

/// A single input action in a sequence
#[derive(Debug, Clone)]
pub enum InputAction {
    /// Send text string
    Text(String),
    /// Send a special key
    Key(Key),
    /// Send raw bytes
    Bytes(Vec<u8>),
    /// Wait for a duration
    Wait(Duration),
}

/// Builder for input sequences
#[derive(Debug, Clone, Default)]
pub struct InputSequence {
    actions: Vec<InputAction>,
}

impl InputSequence {
    /// Create a new empty input sequence
    pub fn new() -> Self {
        Self::default()
    }

    /// Add text to the sequence
    pub fn text(mut self, s: impl Into<String>) -> Self {
        self.actions.push(InputAction::Text(s.into()));
        self
    }

    /// Add a line of text followed by Enter
    pub fn line(mut self, s: impl Into<String>) -> Self {
        self.actions.push(InputAction::Text(s.into()));
        self.actions.push(InputAction::Key(Key::Enter));
        self
    }

    /// Add a key press
    pub fn key(mut self, key: Key) -> Self {
        self.actions.push(InputAction::Key(key));
        self
    }

    /// Add Enter key
    pub fn enter(self) -> Self {
        self.key(Key::Enter)
    }

    /// Add Tab key
    pub fn tab(self) -> Self {
        self.key(Key::Tab)
    }

    /// Add Escape key
    pub fn escape(self) -> Self {
        self.key(Key::Escape)
    }

    /// Add Ctrl+C
    pub fn ctrl_c(self) -> Self {
        self.key(Key::CtrlC)
    }

    /// Add Ctrl+D
    pub fn ctrl_d(self) -> Self {
        self.key(Key::CtrlD)
    }

    /// Add Up arrow
    pub fn up(self) -> Self {
        self.key(Key::Up)
    }

    /// Add Down arrow
    pub fn down(self) -> Self {
        self.key(Key::Down)
    }

    /// Add Left arrow
    pub fn left(self) -> Self {
        self.key(Key::Left)
    }

    /// Add Right arrow
    pub fn right(self) -> Self {
        self.key(Key::Right)
    }

    /// Add raw bytes
    pub fn bytes(mut self, b: impl Into<Vec<u8>>) -> Self {
        self.actions.push(InputAction::Bytes(b.into()));
        self
    }

    /// Add a wait/delay
    pub fn wait(mut self, duration: Duration) -> Self {
        self.actions.push(InputAction::Wait(duration));
        self
    }

    /// Add a wait in milliseconds
    pub fn wait_ms(self, ms: u64) -> Self {
        self.wait(Duration::from_millis(ms))
    }

    /// Get the actions in this sequence
    pub fn actions(&self) -> &[InputAction] {
        &self.actions
    }

    /// Execute this input sequence on a PTY runner
    pub fn execute(&self, runner: &mut PtyRunner) -> Result<(), PtyRunnerError> {
        for action in &self.actions {
            match action {
                InputAction::Text(s) => runner.send_input(s)?,
                InputAction::Key(k) => runner.send_key(*k)?,
                InputAction::Bytes(b) => runner.send_bytes(b)?,
                InputAction::Wait(d) => std::thread::sleep(*d),
            }
        }
        Ok(())
    }
}

/// Convenience function to create an input sequence
pub fn input() -> InputSequence {
    InputSequence::new()
}

#[cfg(test)]
mod tests {
    use crate::{WaitExt, input::*};

    #[test]
    fn test_input_sequence_builder() {
        let seq = input().text("hello").enter().wait_ms(100).key(Key::CtrlC);

        assert_eq!(seq.actions().len(), 4);
        assert!(matches!(&seq.actions()[0], InputAction::Text(s) if s == "hello"));
        assert!(matches!(&seq.actions()[1], InputAction::Key(Key::Enter)));
        assert!(matches!(&seq.actions()[2], InputAction::Wait(_)));
        assert!(matches!(&seq.actions()[3], InputAction::Key(Key::CtrlC)));
    }

    #[test]
    fn test_line_helper() {
        let seq = input().line("command");
        assert_eq!(seq.actions().len(), 2);
        assert!(matches!(&seq.actions()[0], InputAction::Text(s) if s == "command"));
        assert!(matches!(&seq.actions()[1], InputAction::Key(Key::Enter)));
    }

    #[test]
    fn test_arrow_keys() {
        let seq = input().up().down().left().right();
        assert_eq!(seq.actions().len(), 4);
        assert!(matches!(&seq.actions()[0], InputAction::Key(Key::Up)));
        assert!(matches!(&seq.actions()[1], InputAction::Key(Key::Down)));
        assert!(matches!(&seq.actions()[2], InputAction::Key(Key::Left)));
        assert!(matches!(&seq.actions()[3], InputAction::Key(Key::Right)));
    }

    #[test]
    fn test_execute_on_runner() {
        let mut runner = PtyRunner::new();
        runner.spawn("cat", &[]).expect("spawn failed");

        let seq = input().text("test").enter();
        seq.execute(&mut runner).expect("execute failed");

        runner
            .wait_for_text_timeout("test", Duration::from_secs(2))
            .expect("input echo should arrive before the deadline");

        runner.send_key(Key::CtrlD).expect("ctrl-d failed");
    }
}
