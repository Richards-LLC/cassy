mod ci_watch;
mod client_input;
mod cloud;
mod delivery;
#[cfg(test)]
mod delivery_matrix_tests;
mod gui_client;
mod lifecycle;
mod output;
mod queue_and_events;
pub(super) mod relay;
pub(crate) mod teams;
mod ws_client;

/// cas-ac7e (GH #130): the daemon struct holds outstanding urgent wake probes,
/// so their type has to be nameable one level up.
pub(crate) use queue_and_events::{InboxDeferredWrite, UrgentWakeProbe};
