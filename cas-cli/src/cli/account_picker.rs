//! Provider-agnostic account-profile machinery shared by `cas claude` and
//! `cas codex`.
//!
//! Both launchers answer the same questions — which accounts exist, which one
//! is active, should we stop and ask, and how does a brand new profile get
//! created without leaking credentials into it. Only three things genuinely
//! differ per provider: the directory prefix (`.claude-` / `.codex-`), how login
//! state is probed, and which files make up the shared configuration surface.
//! Those stay with the provider; everything else lives here so the two cannot
//! drift apart (cas-898d + cas-9cc3, EPIC cas-951b).
//!
//! Hoisted from `cli/claude.rs` after cas-898d merged, at that task owner's
//! recommendation — every item below was a private `fn`/`struct` there, so
//! sharing required moving it either way.

use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub(crate) use crate::account_registry::LoginState;

/// Bound provider status subprocesses and reap a wedged child. Probes capture
/// both streams so provider diagnostics cannot pollute `accounts list --json`.
pub(crate) fn probe_output(
    mut command: std::process::Command,
    timeout: std::time::Duration,
) -> Option<std::process::Output> {
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().ok()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    child.wait_with_output().ok()
}

/// One detected account profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Profile {
    pub(crate) name: String,
    pub(crate) display_name: String,
    pub(crate) directory: PathBuf,
    pub(crate) login_state: LoginState,
    pub(crate) active: bool,
    pub(crate) disabled: bool,
    /// Live usage for the picker, e.g. `5h limit hit, resets 15:00` (cas-e1f4).
    pub(crate) usage: Option<String>,
    pub(crate) recommendation: Option<String>,
    pub(crate) exclusion: Option<String>,
}

/// Sibling directories that share a profile prefix without being accounts.
///
/// Lock and scratch directories are created next to a profile by other tooling
/// (`~/.claude-support@example.com.lock`). Listing one as a selectable account
/// offers the operator a choice that cannot work.
const NON_ACCOUNT_SUFFIXES: [&str; 5] = [".lock", ".tmp", ".bak", ".old", ".backup"];

/// Whether a `<prefix><name>` directory names a real account profile.
pub(crate) fn is_account_profile_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let lowered = name.to_ascii_lowercase();
    if NON_ACCOUNT_SUFFIXES
        .iter()
        .any(|suffix| lowered.ends_with(suffix))
    {
        return false;
    }
    // Dotted scratch names like `foo.bak.1777306474` are not accounts either,
    // but `support@petrastella.io` must survive: only reject a marker segment.
    !lowered
        .rsplit('.')
        .any(|segment| matches!(segment, "lock" | "tmp" | "bak" | "old" | "backup"))
}

/// How one provider names its profile directories.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ProfileLayout {
    /// Directory name of the default account, e.g. `.claude` / `.codex`.
    pub(crate) main_dir: &'static str,
    /// Prefix of a named account directory, e.g. `.claude-` / `.codex-`.
    pub(crate) named_prefix: &'static str,
    /// Reserved name of the default profile, e.g. `main`.
    pub(crate) main_name: &'static str,
}

impl ProfileLayout {
    /// Resolve a convention-based profile name under `home`.
    pub(crate) fn profile_dir(&self, home: &Path, profile: &str) -> PathBuf {
        if profile == self.main_name {
            home.join(self.main_dir)
        } else {
            home.join(format!("{}{profile}", self.named_prefix))
        }
    }
}

/// Detect every account profile under `home`, newest state first-hand.
///
/// The login probe is injected so each provider keeps its own (Claude reads
/// JSON from `claude auth status`; Codex reads the output and exit code of
/// `codex login status`). Nothing here assumes a probe shape.
pub(crate) fn scan_profiles_with(
    layout: ProfileLayout,
    home: &Path,
    active_dir: Option<&Path>,
    mut login_state_for: impl FnMut(&str, &Path) -> LoginState,
) -> io::Result<Vec<Profile>> {
    let provider = match layout.named_prefix {
        ".claude-" => crate::accounts::Provider::Claude,
        ".codex-" => crate::accounts::Provider::Codex,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unknown account provider",
            ));
        }
    };
    let accounts =
        crate::account_registry::discover_with(provider, home, true, &mut login_state_for)?;
    Ok(accounts
        .into_iter()
        .map(|account| Profile {
            disabled: account.flags.disabled,
            active: active_dir.map_or(account.name == layout.main_name, |dir| {
                dir == account.folder
            }),
            name: account.name,
            display_name: account.display_name,
            directory: account.folder,
            login_state: account.login_state,
            usage: None,
            recommendation: None,
            exclusion: None,
        })
        .collect())
}

/// Use structured registry usage for both the displayed summary and policy.
/// Discovery remains selectable even when a provider cannot report capacity.
pub(crate) fn with_usage(provider: crate::accounts::Provider, home: &Path, mut profiles: Vec<Profile>) -> Vec<Profile> {
    let Ok(accounts) = crate::account_registry::collect_for(provider, home, true) else { return profiles };
    let policy = crate::account_selection::load_policy(crate::store::find_cas_root().ok().as_deref());
    apply_recommendation(&mut profiles, &accounts, provider, chrono::Utc::now(), &policy);
    profiles
}

fn apply_recommendation(profiles: &mut [Profile], accounts: &[crate::account_registry::Account], provider: crate::accounts::Provider,
    now: chrono::DateTime<chrono::Utc>, policy: &crate::config::AccountsConfig) {
    let selection = crate::account_selection::recommend(accounts, provider, now, policy);
    for profile in profiles {
        if let Some(account) = accounts.iter().find(|a| a.folder == profile.directory) {
            profile.login_state = account.login_state;
            profile.usage = crate::cli::limits::summarize_windows(&account.windows);
        }
        profile.recommendation = selection.recommended.as_ref()
            .filter(|r| r.account.folder == profile.directory).map(|r| r.reason.clone());
        profile.exclusion = selection.excluded.iter().find(|(a, _)| a.folder == profile.directory).map(|(_, why)| why.clone());
    }
}

/// Return whether a launch should ask the operator to select an account.
///
/// Gating on DETECTED rather than confirmed-logged-in profiles is deliberate: a
/// probe failure must not silently collapse the picker into a default launch.
/// Even one detected account needs the picker because its new-login row is the
/// discoverable path for adding the first named profile. An explicit profile
/// always wins, and pipe/script launches are left alone — a prompt there would
/// hang or eat input meant for the provider CLI.
pub(crate) fn should_prompt_for_profile(
    explicit_profile: Option<&str>,
    profiles: &[Profile],
    stdin_is_terminal: bool,
    stdout_is_terminal: bool,
) -> bool {
    explicit_profile.is_none() && stdin_is_terminal && stdout_is_terminal && !profiles.is_empty()
}

/// One selectable row in the account picker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ProfileChoice {
    Existing { name: String, label: String },
    NewLogin,
}

impl std::fmt::Display for ProfileChoice {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProfileChoice::Existing { label, .. } => formatter.write_str(label),
            ProfileChoice::NewLogin => formatter.write_str("+ Log in a new account…"),
        }
    }
}

pub(crate) fn login_state_label(state: LoginState) -> &'static str {
    match state {
        LoginState::LoggedIn => "logged in",
        LoginState::LoggedOut => "not logged in",
        LoginState::Unknown => "login state unknown",
        LoginState::NeverLoggedIn => "not logged in",
        LoginState::StaleLogin => "not logged in",
    }
}

/// Build the picker rows: every detected profile, then the new-login entry.
///
/// Nothing is filtered out here. A logged-out or unknown account stays
/// selectable and is labelled as such, so the operator sees the real state of
/// the box instead of a silently shortened list.
pub(crate) fn profile_choices(profiles: &[Profile]) -> Vec<ProfileChoice> {
    let mut choices: Vec<ProfileChoice> = profiles
        .iter()
        .map(|profile| {
            let alias = if profile.display_name == profile.name {
                String::new()
            } else {
                format!(" ({})", profile.name)
            };
            let state = format!(
                " — {}{}{}",
                login_state_label(profile.login_state),
                if profile.active { ", active" } else { "" },
                if profile.disabled { ", disabled" } else { "" }
            );
            let display = shorten_picker_text(
                &profile.display_name,
                76usize.saturating_sub(text_width(&alias) + text_width(&state)),
            );
            let mut label = format!("{display}{alias}{state}");
            let policy_detail = profile.recommendation.as_deref()
                .map(|reason| format!(", recommended: {reason}"))
                .or_else(|| profile.exclusion.as_deref().filter(|_| !profile.disabled)
                    .map(|reason| format!(", {reason}")));
            if let Some(detail) = policy_detail {
                label.push_str(&shorten_picker_text(&detail, 76usize.saturating_sub(text_width(&label))));
            } else if let Some(usage) = &profile.usage {
                let compact =
                    usage.replace("reset, usage unknown until next session", "reset/unknown");
                let mut parts: Vec<_> = compact.split(", ").collect();
                parts.sort_by_key(|part| !part.contains("reset/unknown"));
                let compact = parts.join(", ");
                label.push_str(&shorten_picker_text(
                    &format!(": {compact}"),
                    76usize.saturating_sub(text_width(&label)),
                ));
            }
            ProfileChoice::Existing {
                name: profile.name.clone(),
                label: disabled_label(profile.disabled, label),
            }
        })
        .collect();
    choices.push(ProfileChoice::NewLogin);
    choices
}

fn disabled_label(disabled: bool, label: String) -> String {
    use std::io::IsTerminal;
    if disabled && std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none() {
        format!("\x1b[2m{label}\x1b[0m")
    } else {
        label
    }
}

fn text_width(value: &str) -> usize {
    ratatui::text::Line::from(value).width()
}

fn shorten_picker_text(value: &str, width: usize) -> String {
    if text_width(value) <= width {
        return value.to_string();
    }
    if width < 3 {
        return String::new();
    }
    let mut result = String::new();
    for c in value.chars() {
        if text_width(&result) + text_width(&c.to_string()) + 3 > width {
            break;
        }
        result.push(c);
    }
    result.push_str("...");
    result
}

/// Index of the account the environment currently points at, for cursor start.
pub(crate) fn active_choice_index(profiles: &[Profile]) -> usize {
    profiles
        .iter()
        .position(|profile| profile.recommendation.is_some())
        .or_else(|| profiles.iter().position(|profile| profile.active))
        .unwrap_or(0)
}

/// Show the whole list when it plausibly fits.
///
/// inquire's default page is 7. This operator has 7 accounts, which pushed
/// "log in a new account" below the fold — an option you must scroll to find
/// is an option most people never discover.
pub(crate) fn picker_page_size(choice_count: usize) -> usize {
    choice_count.clamp(7, 20)
}

/// What the operator picked: an existing account, or a validated new name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Selection {
    Existing(String),
    NewLogin(String),
}

/// Whether an operator-entered account name can become `<prefix><name>`.
pub(crate) fn validate_new_profile_name(
    name: &str,
    main_name: &str,
) -> std::result::Result<(), String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("enter an email address or account name".to_string());
    }
    if trimmed == main_name {
        return Err(format!(
            "`{main_name}` is the default profile; pick another name"
        ));
    }
    if trimmed
        .chars()
        .any(|c| c.is_whitespace() || c == '/' || c == '\\' || c == '\0')
    {
        return Err("account name cannot contain whitespace or path separators".to_string());
    }
    if trimmed.starts_with('.') {
        return Err("account name cannot start with a dot".to_string());
    }
    if trimmed.starts_with('-') {
        return Err("account name cannot start with a dash".to_string());
    }
    if !is_account_profile_name(trimmed) {
        return Err("that name is reserved for lock/scratch directories".to_string());
    }
    Ok(())
}

/// Prompt for an account, offering every detected profile plus a new login.
///
/// Returns the operator's choice rather than acting on it: creating and signing
/// in a new account is the one step that genuinely differs per provider, so it
/// belongs to the caller.
pub(crate) fn prompt_for_selection(
    provider_label: &str,
    layout: ProfileLayout,
    profiles: &[Profile],
) -> Result<Selection> {
    let choices = profile_choices(profiles);
    let starting_cursor = active_choice_index(profiles);
    let page_size = picker_page_size(choices.len());

    let selection = inquire::Select::new(&format!("Choose {provider_label} account"), choices)
        .with_starting_cursor(starting_cursor)
        .with_page_size(page_size)
        .with_help_message(&format!(
            "Session only. Full aliases and usage: cas {} --list-profiles",
            provider_label.to_ascii_lowercase()
        ))
        .prompt()
        .with_context(|| format!("{provider_label} account selection cancelled"))?;

    match selection {
        ProfileChoice::Existing { name, .. } => Ok(Selection::Existing(name)),
        ProfileChoice::NewLogin => {
            let main_name = layout.main_name.to_string();
            let entered =
                inquire::Text::new(&format!("Email for the new {provider_label} account"))
                    .with_help_message(&format!(
                        "creates ~/{}<email> and runs the {provider_label} login flow",
                        layout.named_prefix
                    ))
                    .with_validator(move |input: &str| {
                        Ok(match validate_new_profile_name(input, &main_name) {
                            Ok(()) => inquire::validator::Validation::Valid,
                            Err(message) => inquire::validator::Validation::Invalid(
                                inquire::validator::ErrorMessage::Custom(message),
                            ),
                        })
                    })
                    .prompt()
                    .with_context(|| format!("new {provider_label} login cancelled"))?;
            Ok(Selection::NewLogin(entered.trim().to_string()))
        }
    }
}

/// What one seeding pass did, so the caller can report it honestly.
#[derive(Debug, Default, Eq, PartialEq)]
pub(crate) struct SeedingReport {
    pub(crate) linked: Vec<String>,
    pub(crate) already_linked: Vec<String>,
    pub(crate) skipped_existing: Vec<String>,
    pub(crate) missing_in_main: Vec<String>,
}

/// Idempotently symlink a shared config surface from the main profile.
///
/// Re-running is safe and never clobbers: an entry the operator later replaced
/// with their own real file or a different link is reported and left alone.
///
/// `private` is not merely documentation — it is asserted against `shared`,
/// because that assertion is the only thing standing between this helper and
/// quietly symlinking someone's credentials into another account.
pub(crate) fn seed_profile_dir(
    main_dir: &Path,
    profile_dir: &Path,
    shared: &[&str],
    private: &[&str],
) -> Result<SeedingReport> {
    let mut report = SeedingReport::default();
    if main_dir == profile_dir || !main_dir.is_dir() {
        return Ok(report);
    }

    for entry in shared {
        debug_assert!(
            !private.contains(entry),
            "credential/identity state must never be shared"
        );
        if private.contains(entry) {
            // Belt and braces: a release build must refuse too, not just debug.
            continue;
        }
        let source = main_dir.join(entry);
        if !source.exists() {
            report.missing_in_main.push((*entry).to_string());
            continue;
        }
        let target = profile_dir.join(entry);
        match std::fs::symlink_metadata(&target) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                symlink_entry(&source, &target).with_context(|| {
                    format!("could not link {} into {}", entry, profile_dir.display())
                })?;
                report.linked.push((*entry).to_string());
            }
            Err(error) => return Err(error).context(format!("could not inspect {entry}")),
            Ok(metadata) => {
                if metadata.file_type().is_symlink()
                    && std::fs::read_link(&target).is_ok_and(|existing| existing == source)
                {
                    report.already_linked.push((*entry).to_string());
                } else {
                    report.skipped_existing.push((*entry).to_string());
                }
            }
        }
    }
    Ok(report)
}

#[cfg(unix)]
fn symlink_entry(source: &Path, target: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(source, target)
}

#[cfg(not(unix))]
fn symlink_entry(source: &Path, target: &Path) -> io::Result<()> {
    if source.is_dir() {
        std::os::windows::fs::symlink_dir(source, target)
    } else {
        std::os::windows::fs::symlink_file(source, target)
    }
}

/// Tell the operator what a new profile inherited, and what it did not.
pub(crate) fn report_seeding(report: &SeedingReport, source_label: &str) {
    if !report.linked.is_empty() {
        eprintln!(
            "Seeded shared config from {source_label}: {}",
            report.linked.join(", ")
        );
    }
    if !report.skipped_existing.is_empty() {
        eprintln!(
            "Left existing entries untouched: {}",
            report.skipped_existing.join(", ")
        );
    }
    eprintln!("Credentials and account identity remain private to this profile.");
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const CLAUDE: ProfileLayout = ProfileLayout {
        main_dir: ".claude",
        named_prefix: ".claude-",
        main_name: "main",
    };
    const CODEX: ProfileLayout = ProfileLayout {
        main_dir: ".codex",
        named_prefix: ".codex-",
        main_name: "main",
    };

    #[test]
    fn layout_resolves_both_providers_by_the_same_rule() {
        let home = Path::new("/tmp/test-home");

        assert_eq!(CLAUDE.profile_dir(home, "main"), home.join(".claude"));
        assert_eq!(CLAUDE.profile_dir(home, "alt"), home.join(".claude-alt"));
        assert_eq!(CODEX.profile_dir(home, "main"), home.join(".codex"));
        assert_eq!(
            CODEX.profile_dir(home, "daniel@petrastella.io"),
            home.join(".codex-daniel@petrastella.io")
        );
    }

    #[test]
    fn lock_and_scratch_siblings_are_not_accounts_but_dotted_emails_are() {
        assert!(is_account_profile_name("support@petrastella.io"));
        assert!(is_account_profile_name("alt"));

        for rejected in [
            "",
            "support@petrastella.io.lock",
            "work.bak",
            "work.tmp.1777306474",
            "old-profile.old",
        ] {
            assert!(
                !is_account_profile_name(rejected),
                "{rejected:?} should not be treated as an account"
            );
        }
    }

    #[test]
    fn scan_is_provider_parameterised_and_marks_the_active_account() {
        let home = TempDir::new().unwrap();
        for dir in [".codex", ".codex-alt", ".codex-work", ".codex-scratch.lock"] {
            std::fs::create_dir_all(home.path().join(dir)).unwrap();
        }
        // A Claude directory must not appear in a Codex scan.
        std::fs::create_dir_all(home.path().join(".claude-alt")).unwrap();

        let alt = home.path().join(".codex-alt");
        use base64::Engine as _;
        let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"email":"operator@example.com"}"#);
        std::fs::write(
            alt.join("auth.json"),
            serde_json::json!({
                "tokens": {"id_token": format!("header.{claims}.signature")}
            })
            .to_string(),
        )
        .unwrap();
        std::fs::copy(
            alt.join("auth.json"),
            home.path().join(".codex-work/auth.json"),
        )
        .unwrap();
        let mut profiles =
            scan_profiles_with(CODEX, home.path(), Some(alt.as_path()), |name, _| {
                if name == "alt" {
                    LoginState::LoggedIn
                } else {
                    LoginState::Unknown
                }
            })
            .unwrap();

        let names: Vec<&str> = profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["main", "alt", "work"]);
        assert!(profiles[1].active);
        assert!(!profiles[0].active);
        profiles[2].usage = Some("weekly 99%, 5h reset, usage unknown until next session".into());
        let rows = profile_choices(&profiles);
        assert!(
            rows[1]
                .to_string()
                .starts_with("operator@example.com (alt) — logged in")
        );
        assert!(matches!(&rows[1], ProfileChoice::Existing { name, .. } if name == "alt"));
        assert!(
            rows[2]
                .to_string()
                .starts_with("operator@example.com (work) —")
        );
        assert_ne!(rows[1].to_string(), rows[2].to_string());
        assert!(rows[2].to_string().contains("reset/unknown"));
        assert!(rows.iter().all(|row| text_width(&row.to_string()) <= 76));
    }

    #[test]
    fn prompting_needs_an_interactive_terminal_but_not_multiple_profiles() {
        let profiles = vec![
            Profile {
                name: "main".to_string(),
                display_name: "main".to_string(),
                directory: PathBuf::from("/tmp/.codex"),
                login_state: LoginState::LoggedIn,
                active: true,
                disabled: false,
                usage: None,
            recommendation: None,
            exclusion: None,
            },
            Profile {
                name: "alt".to_string(),
                display_name: "alt".to_string(),
                directory: PathBuf::from("/tmp/.codex-alt"),
                login_state: LoginState::Unknown,
                active: false,
                disabled: false,
                usage: None,
            recommendation: None,
            exclusion: None,
            },
        ];

        assert!(should_prompt_for_profile(None, &profiles, true, true));
        assert!(!should_prompt_for_profile(None, &profiles, false, true));
        assert!(!should_prompt_for_profile(None, &profiles, true, false));
        assert!(!should_prompt_for_profile(
            Some("alt"),
            &profiles,
            true,
            true
        ));
        assert!(should_prompt_for_profile(None, &profiles[..1], true, true));
        assert!(!should_prompt_for_profile(None, &[], true, true));
        assert!(!should_prompt_for_profile(None, &profiles, false, false));
    }

    #[test]
    fn every_detected_account_is_offered_with_its_state_plus_a_new_login_row() {
        let profiles = vec![
            Profile {
                name: "main".to_string(),
                display_name: "main".to_string(),
                directory: PathBuf::from("/tmp/.codex"),
                login_state: LoginState::LoggedOut,
                active: true,
                disabled: false,
                usage: None,
            recommendation: None,
            exclusion: None,
            },
            Profile {
                name: "alt".to_string(),
                display_name: "alt".to_string(),
                directory: PathBuf::from("/tmp/.codex-alt"),
                login_state: LoginState::Unknown,
                active: false,
                disabled: true,
                usage: None,
            recommendation: None,
            exclusion: None,
            },
        ];

        let choices = profile_choices(&profiles);
        assert_eq!(choices.len(), 3);
        assert_eq!(choices[0].to_string(), "main — not logged in, active");
        assert_eq!(
            choices[1].to_string(),
            "alt — login state unknown, disabled"
        );
        assert_eq!(choices[2], ProfileChoice::NewLogin);
        assert_eq!(active_choice_index(&profiles), 0);
        for provider in crate::accounts::Provider::ALL {
            let accounts = vec![
                crate::account_selection::tests::row(provider, "main", 48, 0.0, 0.0),
                crate::account_selection::tests::row(provider, "alt", 6, 59.0, 20.0),
            ];
            let mut profiles = profiles.clone();
            for (profile, account) in profiles.iter_mut().zip(&accounts) { profile.directory = account.folder.clone(); }
            apply_recommendation(&mut profiles, &accounts, provider, "2026-10-27T12:00:00Z".parse().unwrap(), &Default::default());
            let choices = profile_choices(&profiles);
            assert_eq!(active_choice_index(&profiles), 1);
            assert!(choices[1].to_string().contains("recommended: resets in 6h, 41% left"));
            assert!(choices[0].to_string().contains("resets later than alt"));
            let mut idle_accounts = accounts.clone();
            idle_accounts[1].windows.remove(0);
            idle_accounts[1].windows[0].used_percent = Some(10.0);
            idle_accounts[0].flags.duplicate_of = Some(idle_accounts[1].folder.clone());
            apply_recommendation(&mut profiles, &idle_accounts, provider, "2026-10-27T12:00:00Z".parse().unwrap(), &Default::default());
            let idle_choices = profile_choices(&profiles);
            assert_eq!(active_choice_index(&profiles), 1);
            assert!(idle_choices[1].to_string().contains("recommended: resets in 6h, 90% left"));
            assert!(idle_choices[0].to_string().contains("duplicate of alt"));
            let mut unknown_reset_accounts = accounts.clone();
            unknown_reset_accounts[1].windows[1].resets_at = None;
            apply_recommendation(&mut profiles, &unknown_reset_accounts, provider, "2026-10-27T12:00:00Z".parse().unwrap(), &Default::default());
            assert!(profile_choices(&profiles)[1].to_string().contains("weekly reset unknown; known resets first"));
            unknown_reset_accounts[0].flags.disabled = true;
            apply_recommendation(&mut profiles, &unknown_reset_accounts, provider, "2026-10-27T12:00:00Z".parse().unwrap(), &Default::default());
            assert!(profile_choices(&profiles)[1].to_string().contains("weekly reset unknown, 41% left"));

            assert!(matches!(&choices[0], ProfileChoice::Existing { name, .. } if name == "main"));
            assert!(matches!(&choices[1], ProfileChoice::Existing { name, .. } if name == "alt"));
            assert_eq!(choices[2], ProfileChoice::NewLogin);
        }

    }

    #[test]
    fn the_new_login_row_is_never_hidden_below_the_fold() {
        // 7 accounts + the new-login row must all be visible at once.
        assert_eq!(picker_page_size(8), 8);
        assert_eq!(picker_page_size(3), 7);
        assert_eq!(picker_page_size(50), 20);
    }

    #[test]
    fn new_profile_names_that_could_escape_or_collide_are_refused() {
        assert!(validate_new_profile_name("work@example.com", "main").is_ok());

        for bad in [
            "",
            "   ",
            "main",
            "../escape",
            "a/b",
            "a b",
            ".hidden",
            "-flag",
            "x.lock",
        ] {
            assert!(
                validate_new_profile_name(bad, "main").is_err(),
                "{bad:?} should be refused as a profile directory name"
            );
        }
    }

    #[test]
    fn seeding_links_shared_entries_is_idempotent_and_refuses_private_ones() {
        let home = TempDir::new().unwrap();
        let main = home.path().join(".codex");
        let profile = home.path().join(".codex-work@example.com");
        std::fs::create_dir_all(main.join("skills")).unwrap();
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(main.join("config.toml"), "model = \"gpt-5\"\n").unwrap();
        std::fs::write(main.join("auth.json"), "{\"secret\":true}").unwrap();

        let shared = ["config.toml", "skills", "AGENTS.md"];
        let private = ["auth.json"];

        let first = seed_profile_dir(&main, &profile, &shared, &private).unwrap();
        assert_eq!(first.linked, vec!["config.toml", "skills"]);
        assert_eq!(first.missing_in_main, vec!["AGENTS.md"]);
        assert_eq!(
            std::fs::read_to_string(profile.join("config.toml")).unwrap(),
            "model = \"gpt-5\"\n"
        );

        // Re-seeding recognises its own links and leaves a diverged file alone.
        std::fs::remove_file(profile.join("config.toml")).unwrap();
        std::fs::write(profile.join("config.toml"), "model = \"mine\"\n").unwrap();
        let second = seed_profile_dir(&main, &profile, &shared, &private).unwrap();
        assert!(second.linked.is_empty());
        assert_eq!(second.already_linked, vec!["skills"]);
        assert_eq!(second.skipped_existing, vec!["config.toml"]);
        assert_eq!(
            std::fs::read_to_string(profile.join("config.toml")).unwrap(),
            "model = \"mine\"\n"
        );

        // No private entry ever lands in the seeded profile.
        for entry in private {
            assert!(
                !profile.join(entry).exists(),
                "{entry} must stay private to a profile"
            );
        }
    }

    #[test]
    fn a_private_entry_smuggled_into_the_shared_list_is_still_not_linked() {
        // debug_assert catches this in development; release builds must refuse
        // it too rather than symlinking credentials.
        let home = TempDir::new().unwrap();
        let main = home.path().join(".codex");
        let profile = home.path().join(".codex-alt");
        std::fs::create_dir_all(&main).unwrap();
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(main.join("auth.json"), "{\"secret\":true}").unwrap();

        let result = std::panic::catch_unwind(|| {
            seed_profile_dir(&main, &profile, &["auth.json"], &["auth.json"]).map(|r| r.linked)
        });

        // Debug builds trip the assertion; either way the file is not linked.
        if let Ok(Ok(linked)) = result {
            assert!(linked.is_empty());
        }
        assert!(!profile.join("auth.json").exists());
    }
}
