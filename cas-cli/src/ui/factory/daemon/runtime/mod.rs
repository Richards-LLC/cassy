mod ci_watch;
mod client_input;
mod cloud;
mod commander_mirror;
pub(crate) mod delivery;
pub(crate) mod director_refresh;
#[cfg(test)]
mod delivery_matrix_tests;
mod gui_client;
mod injection_events;
mod lifecycle;
#[cfg(test)]
mod loop_latency_tests;
pub(crate) mod loop_watchdog;
pub(super) mod merge_sweep;
mod output;
pub(super) mod pane_size;
#[cfg(test)]
mod provisioning_tests;
pub mod queue_and_events;
pub(super) mod relay;
pub(crate) mod send_dedupe;
pub(super) mod session_summarizer;
pub(crate) mod store_worker;
pub(crate) mod teams;
pub(super) mod terminal_exchange;
pub(crate) mod violet_activity;
mod ws_client;

/// cas-ac7e (GH #130): the daemon struct holds outstanding urgent wake probes,
/// so their type has to be nameable one level up.
pub(crate) use queue_and_events::{
    InboxDeferredWrite, NormalDeliveryProbe, ObservedWorkerExit, UrgentWakeProbe,
};
