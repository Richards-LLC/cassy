mod get;
mod global;
mod hooks_traits;
pub(crate) mod io;
mod list;
mod notices;
mod retirement;
mod set;

pub(crate) use set::validate_notification_webhook_url;

pub(crate) use retirement::{ConfigRetirement, retire_config};
pub(crate) use notices::{RetiredConfigNotice, RetiredNoticeScope, represented_retired_config};

pub use global::{
    get_telemetry_consent, global_cas_dir, load_global_config, prompt_telemetry_consent,
    save_global_config, set_telemetry_consent,
};
