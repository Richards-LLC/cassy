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

/// Whether a profile's credential is usable right now.
///
/// `Unknown` is a first-class answer, not an error: a probe that cannot reach
/// the provider CLI must leave the account visible and selectable rather than
/// silently shortening the operator's list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LoginState {
    LoggedIn,
    LoggedOut,
    Unknown,
}

/// One detected account profile.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Profile {
    pub(crate) name: String,
    pub(crate) directory: PathBuf,
    pub(crate) login_state: LoginState,
    pub(crate) active: bool,
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
    let mut profiles = vec![profile_for(
        layout.main_name,
        home.join(layout.main_dir),
        active_dir,
        layout.main_name,
        &mut login_state_for,
    )];

    if let Ok(entries) = std::fs::read_dir(home) {
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some(profile_name) = file_name.strip_prefix(layout.named_prefix) else {
                continue;
            };
            if !is_account_profile_name(profile_name) {
                continue;
            }
            let profile_name = profile_name.to_string();
            profiles.push(profile_for(
                &profile_name,
                path,
                active_dir,
                layout.main_name,
                &mut login_state_for,
            ));
        }
    }

    profiles[1..].sort_by(|left, right| left.name.cmp(&right.name));
    Ok(profiles)
}

fn profile_for(
    name: &str,
    directory: PathBuf,
    active_dir: Option<&Path>,
    main_name: &str,
    login_state_for: &mut impl FnMut(&str, &Path) -> LoginState,
) -> Profile {
    Profile {
        name: name.to_string(),
        login_state: login_state_for(name, &directory),
        active: active_dir.map_or(name == main_name, |active| active == directory),
        directory,
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
        .map(|profile| ProfileChoice::Existing {
            name: profile.name.clone(),
            label: format!(
                "{} — {}{}",
                profile.name,
                login_state_label(profile.login_state),
                if profile.active { ", active" } else { "" }
            ),
        })
        .collect();
    choices.push(ProfileChoice::NewLogin);
    choices
}

/// Index of the account the environment currently points at, for cursor start.
pub(crate) fn active_choice_index(profiles: &[Profile]) -> usize {
    profiles
        .iter()
        .position(|profile| profile.active)
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
            "This selection applies only to this {provider_label} session"
        ))
        .prompt()
        .with_context(|| format!("{provider_label} account selection cancelled"))?;

    match selection {
        ProfileChoice::Existing { name, .. } => Ok(Selection::Existing(name)),
        ProfileChoice::NewLogin => {
            let main_name = layout.main_name.to_string();
            let entered = inquire::Text::new(&format!("Email for the new {provider_label} account"))
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
        let profiles =
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
    }

    #[test]
    fn prompting_needs_an_interactive_terminal_but_not_multiple_profiles() {
        let profiles = vec![
            Profile {
                name: "main".to_string(),
                directory: PathBuf::from("/tmp/.codex"),
                login_state: LoginState::LoggedIn,
                active: true,
            },
            Profile {
                name: "alt".to_string(),
                directory: PathBuf::from("/tmp/.codex-alt"),
                login_state: LoginState::Unknown,
                active: false,
            },
        ];

        assert!(should_prompt_for_profile(None, &profiles, true, true));
        assert!(!should_prompt_for_profile(None, &profiles, false, true));
        assert!(!should_prompt_for_profile(None, &profiles, true, false));
        assert!(!should_prompt_for_profile(Some("alt"), &profiles, true, true));
        assert!(should_prompt_for_profile(None, &profiles[..1], true, true));
    }

    #[test]
    fn every_detected_account_is_offered_with_its_state_plus_a_new_login_row() {
        let profiles = vec![
            Profile {
                name: "main".to_string(),
                directory: PathBuf::from("/tmp/.codex"),
                login_state: LoginState::LoggedOut,
                active: true,
            },
            Profile {
                name: "alt".to_string(),
                directory: PathBuf::from("/tmp/.codex-alt"),
                login_state: LoginState::Unknown,
                active: false,
            },
        ];

        let choices = profile_choices(&profiles);
        assert_eq!(choices.len(), 3);
        assert_eq!(choices[0].to_string(), "main — not logged in, active");
        assert_eq!(choices[1].to_string(), "alt — login state unknown");
        assert_eq!(choices[2], ProfileChoice::NewLogin);
        assert_eq!(active_choice_index(&profiles), 0);
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

        for bad in ["", "   ", "main", "../escape", "a/b", "a b", ".hidden", "-flag", "x.lock"] {
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
