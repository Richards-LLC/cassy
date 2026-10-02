//! One synchronous epic collection owns one deadline, including nested Git
//! proofs. Thread-local scope avoids widening every shared delivery helper's
//! signature; outside the collection their existing behavior is unchanged.
use crate::bounded_process::{BoundedCommandError, Deadline};
use std::cell::Cell;
use std::io;
use std::process::{Command, ExitStatus, Output, Stdio};
use std::time::Duration;

thread_local! {
    static DEADLINE: Cell<Option<Deadline>> = const { Cell::new(None) };
}

pub(super) struct Scope(Option<Deadline>);

impl Scope {
    pub(super) fn new(budget: Duration) -> Self {
        let next = (budget != Duration::MAX).then(|| Deadline::after(budget));
        Self(DEADLINE.replace(next))
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        DEADLINE.set(self.0);
    }
}

pub(super) fn deadline() -> Option<Deadline> {
    DEADLINE.get()
}

pub(super) fn expired() -> bool {
    deadline().is_some_and(|deadline| deadline.remaining().is_zero())
}

pub(super) fn check() -> Result<(), String> {
    if expired() {
        Err("epic delivery measurement budget exhausted".into())
    } else {
        Ok(())
    }
}

fn io_error(error: BoundedCommandError) -> io::Error {
    match error {
        BoundedCommandError::TimedOut => io::Error::new(
            io::ErrorKind::TimedOut,
            "epic delivery measurement budget exhausted",
        ),
        BoundedCommandError::Io => io::Error::other("epic delivery measurement Git probe failed"),
    }
}

pub(super) trait CommandExt {
    fn measurement_output(&mut self) -> io::Result<Output>;
    fn measurement_output_with_stdin(&mut self, stdin: Stdio) -> io::Result<Output>;
    fn measurement_status(&mut self) -> io::Result<ExitStatus>;
}

impl CommandExt for Command {
    fn measurement_output(&mut self) -> io::Result<Output> {
        match deadline() {
            Some(deadline) => {
                crate::bounded_process::run_command(self, deadline, Duration::MAX).map_err(io_error)
            }
            None => self.output(),
        }
    }

    fn measurement_output_with_stdin(&mut self, stdin: Stdio) -> io::Result<Output> {
        match deadline() {
            Some(deadline) => {
                crate::bounded_process::run_command_with_stdin(self, deadline, Duration::MAX, stdin)
                    .map_err(io_error)
            }
            None => self.stdin(stdin).output(),
        }
    }

    fn measurement_status(&mut self) -> io::Result<ExitStatus> {
        match deadline() {
            Some(_) => self.measurement_output().map(|output| output.status),
            None => self.status(),
        }
    }
}
