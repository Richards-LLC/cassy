pub(crate) mod ci_watch;
pub(crate) mod external_wake;
pub(crate) mod merge_queue_ejection;
#[cfg(test)]
mod busy_claude_delivery_tests;
mod client_input;
mod cloud;
mod commander_mirror;
pub(crate) use commander_mirror::MirrorScanState;
pub(crate) mod delivery;
pub(crate) mod director_refresh;
pub(crate) mod external_work_reconcile_jobs;
pub(crate) mod factory_jobs;
pub(crate) mod relay_jobs;
#[cfg(test)]
mod relay_jobs_tests;
#[cfg(test)]
mod delivery_matrix_tests;
mod gui_client;
pub(crate) mod inject_commands;
#[cfg(test)]
mod inject_command_tests;
mod injection_events;
pub(crate) mod lifecycle;
#[cfg(test)]
mod loop_latency_tests;
#[cfg(test)]
mod peer_mailbox_wake_tests;
pub(crate) mod loop_watchdog;
mod output;
pub(super) mod pane_size;
#[cfg(test)]
mod provisioning_tests;
#[cfg(test)]
mod reminder_wake_tests;
pub mod queue_and_events;
pub(super) mod relay;
pub(crate) mod render_scheduler;
pub(crate) mod send_dedupe;
#[cfg(test)]
mod shutdown_tests;
pub(super) mod session_summarizer;
pub(crate) mod store_worker;
pub(crate) mod jobs;
pub(crate) mod prompt_queue_jobs;
pub(crate) mod teams;
pub(super) mod terminal_exchange;
pub(crate) mod violet_activity;
pub(crate) mod violet_consumer;
pub(crate) mod violet_intake;
pub(crate) mod violet_reader;
mod violet_body;
mod ws_client;

/// cas-ac7e (GH #130): the daemon struct holds outstanding urgent wake probes,
/// so their type has to be nameable one level up.
pub(crate) use queue_and_events::{
    InboxDeferredWrite, NormalDeliveryProbe, ObservedWorkerExit, UrgentWakeProbe,
};

#[cfg(test)]
mod supervisor_notice_tests;

pub(crate) mod queue_control_jobs;

pub(crate) mod delivery_probe_jobs;

pub(crate) mod poison_sweep_jobs;

#[cfg(test)]
mod supervisor_single_turn_tests;
