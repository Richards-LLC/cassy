mod get;
mod global;
mod hooks_traits;
pub(crate) mod io;
mod list;
mod set;

/// GH #963: `issues.components.mecha_cassy` is the deprecated name of
/// `issues.components.violet`, accepted for one release.
fn warn_deprecated_issue_key() {
    eprintln!(
        "warning: issues.components.mecha_cassy is deprecated; use issues.components.violet. \
         The old key is accepted for one release."
    );
}

pub use global::{
    get_telemetry_consent, global_cas_dir, load_global_config, prompt_telemetry_consent,
    save_global_config, set_telemetry_consent,
};
