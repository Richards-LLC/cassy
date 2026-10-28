# crossterm 0.25.0 buffered-event handoff

Cassy vendors the exact crates.io `crossterm` 0.25.0 package under
`vendor/crossterm-0.25.0` and overrides it with `[patch.crates-io]`. The
upstream package checksum was
`e64e6c0fbe2c17357405f7c758c1ef960fce08bdfb2c03d88d2a18d7e09c4b67`.
The upstream MIT license, README and changelog are preserved. Only `inquire`
0.7 depends on this line; `ratatui` and Cassy's own `crossterm` 0.29 resolve
from crates.io unchanged, because the patch's version satisfies only `^0.25`.

## Why

inquire reads keys through crossterm 0.25, which reads up to 1 KiB from the
terminal at once and keeps every parsed key after the prompt's Enter in a
private queue. The account picker's new-login flow asks a later
duplicate-account question that reads standard input, so an answer typed
together with the email never arrived. The public `event::poll` and `read`
cannot hand that queue over safely: when the queue holds no event, `poll`
reads the terminal, which can consume newer input out of order and drop the
empty read that is a canonical-mode EOF.

## The change

One additive public function and its plumbing, in `src/event.rs`,
`src/event/read.rs`, `src/event/source.rs` and `src/event/source/unix.rs`:

- `event::take_buffered_events() -> Vec<Event>` removes and returns, oldest
  first, the public events this process already read but has not delivered:
  the reader's queue, then events the unix source's parser completed.
- It never creates, polls or reads the event source. With no reader it
  returns an empty `Vec` and leaves the reader uncreated.
- Internal events (cursor-position replies) stay queued in their order.
- Bytes of an unfinished escape sequence stay in the parser.

No existing function changed behaviour. Two vendored unit tests cover it: a
fake source whose `try_read` panics, and the uncreated reader. The full diff
against the crates.io package is small enough to review with
`diff -ru <cargo registry>/crossterm-0.25.0 vendor/crossterm-0.25.0`.
