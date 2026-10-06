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

pub(crate) struct Scope(Option<Deadline>);

impl Scope {
    pub(crate) fn new(budget: Duration) -> Self {
        let next = (budget != Duration::MAX).then(|| Deadline::after(budget));
        Self(DEADLINE.replace(next))
    }

    /// cas-bdd2: install a caller's deadline in a worker thread, so probes run
    /// there honour the same budget and expire at the same instant.
    pub(crate) fn inherit(deadline: Option<Deadline>) -> Self {
        Self(DEADLINE.replace(deadline))
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        DEADLINE.set(self.0);
    }
}

pub(crate) fn deadline() -> Option<Deadline> {
    DEADLINE.get()
}

pub(crate) fn expired() -> bool {
    deadline().is_some_and(|deadline| deadline.remaining().is_zero())
}

pub(crate) fn check() -> Result<(), String> {
    if expired() {
        Err("epic delivery measurement budget exhausted".into())
    } else {
        Ok(())
    }
}

/// Thread-local, deterministic load injection; never reads process-wide env.
#[cfg(test)]
pub(crate) mod test_load {
    use std::cell::Cell;
    use std::time::Duration;

    thread_local! {
        static DELAY: Cell<Duration> = const { Cell::new(Duration::ZERO) };
        static CHILD_CAP: Cell<Duration> = const { Cell::new(Duration::from_secs(20)) };
    }

    pub(crate) struct Guard(Duration, Duration);

    impl Guard {
        pub(crate) fn new(delay: Duration, child_cap: Duration) -> Self {
            Self(DELAY.replace(delay), CHILD_CAP.replace(child_cap))
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            DELAY.set(self.0);
            CHILD_CAP.set(self.1);
        }
    }

    pub(crate) fn delay() {
        std::thread::sleep(DELAY.get());
    }

    pub(crate) fn child_cap() -> Duration {
        CHILD_CAP.get()
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

pub(crate) trait CommandExt {
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
