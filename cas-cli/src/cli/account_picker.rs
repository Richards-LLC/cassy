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
    /// Live usage for `--list-profiles`, e.g. `5h limit hit, resets 15:00` (cas-e1f4).
    pub(crate) usage: Option<String>,
    /// The chooser's one capacity line, e.g. `weekly 64% left, resets …`.
    /// `None` means no weekly data was reported; the chooser says why.
    pub(crate) weekly: Option<String>,
    /// Canonical folder of the same provider identity; the chooser hides it.
    pub(crate) duplicate_of: Option<PathBuf>,
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
    // but `support@petrastella.io` must survive: only reject a marker segment
    // after a dot. A whole alias such as `old` or `backup` is a real account.
    !lowered
        .split('.')
        .skip(1)
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
            weekly: None,
            duplicate_of: account.flags.duplicate_of,
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
            profile.usage = crate::cli::limits::summarize_windows_at(&account.windows, now);
            profile.weekly = weekly_line(&account.windows, now);
            profile.duplicate_of = account.flags.duplicate_of.clone();
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
    /// `title` is what the answer line repeats once the operator presses Enter.
    Existing { name: String, title: String, label: String },
    NewLogin,
}

impl std::fmt::Display for ProfileChoice {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProfileChoice::Existing { label, .. } => formatter.write_str(label),
            // An action, not truncated content: no ellipsis, ASCII in any locale.
            ProfileChoice::NewLogin => formatter.write_str("+ Log in a new account"),
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

/// Widest chooser line before inquire's two-cell cursor prefix.
const ROW_WIDTH: usize = 76;
/// One-cell marks measured at >= 3:1 on the dark, light and both Solarized
/// palettes of the terminal-qa gate. Words beside them stay default foreground.
const RECOMMENDED_MARK: (u8, u8, u8) = (40, 145, 75);
const LIMIT_MARK: (u8, u8, u8) = (176, 112, 16);

#[derive(Clone, Copy)]
enum Mark {
    Recommended,
    Limit,
}

/// How the chooser may decorate itself on this terminal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PickerStyle {
    /// UTF-8 glyphs; otherwise the ASCII `[OK]` / `[WARN]` words.
    pub(crate) glyphs: bool,
    /// Any SGR at all: bold selection and coloured marks. Off for NO_COLOR or a pipe.
    pub(crate) styled: bool,
}

impl PickerStyle {
    pub(crate) fn detect() -> Self {
        use std::io::IsTerminal;
        Self {
            glyphs: crate::ui::components::formatter::locale_is_utf8(),
            styled: std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
        }
    }

    fn mark(self, mark: Mark) -> String {
        let (glyph, ascii, (r, g, b)) = match mark {
            Mark::Recommended => ("\u{2713}", "[OK]", RECOMMENDED_MARK),
            Mark::Limit => ("\u{26a0}", "[WARN]", LIMIT_MARK),
        };
        match (self.glyphs, self.styled) {
            (true, true) => format!("\x1b[38;2;{r};{g};{b}m{glyph}\x1b[39m"),
            (true, false) => glyph.to_string(),
            // ASCII words stay uncoloured, as the shared Formatter does.
            (false, _) => ascii.to_string(),
        }
    }
}

/// inquire's colours (cyan selection, green/red marks) fail contrast on light
/// and Solarized themes. Selection is the `>` cursor plus bold in the default
/// foreground; with SGR disabled nothing is styled at all.
fn render_config(style: PickerStyle) -> inquire::ui::RenderConfig<'static> {
    use inquire::ui::{Attributes, RenderConfig, StyleSheet, Styled};
    let mut config = RenderConfig::empty();
    config.answered_prompt_prefix = Styled::new(">");
    if style.styled {
        let bold = StyleSheet::new().with_attr(Attributes::BOLD);
        config.prompt_prefix = Styled::new("?").with_attr(Attributes::BOLD);
        config.answered_prompt_prefix = Styled::new(">").with_attr(Attributes::BOLD);
        config.highlighted_option_prefix = Styled::new(">").with_attr(Attributes::BOLD);
        config.prompt = bold;
        config.selected_option = Some(bold);
    }
    config
}

/// The folders the chooser offers. A duplicate of a listed account and a
/// never-used folder (no credential, email or identity, and a logged-out
/// probe) are hidden. `main` and the active folder always stay; every hidden
/// folder remains launchable by name and listed by `--list-profiles`.
pub(crate) fn chooser_profiles(profiles: &[Profile]) -> Vec<&Profile> {
    profiles
        .iter()
        .filter(|profile| {
            let duplicate = profile.duplicate_of.as_ref().is_some_and(|canonical| {
                profiles.iter().any(|other| &other.directory == canonical)
            });
            let unused = profile.login_state == LoginState::NeverLoggedIn
                && profile.name != "main"
                && !profile.active;
            !duplicate && !unused
        })
        .collect()
}

/// An exclusion that limits this account and is not already said elsewhere:
/// login and disabled state have their own tags, unknown usage is on the
/// weekly line, and comparisons with the recommended row are implied by it.
fn limit_tag(profile: &Profile) -> Option<&str> {
    if profile.recommendation.is_some() {
        return None;
    }
    let reason = profile.exclusion.as_deref()?;
    let reason = reason.split(';').next().unwrap_or(reason).trim();
    let said_elsewhere = reason == crate::account_selection::LOGIN_UNAVAILABLE_REASON
        || matches!(reason, "disabled" | "stale login" | "usage unknown" | "weekly reset unknown")
        || reason.starts_with("duplicate of ")
        || reason.starts_with("resets later than ");
    (!said_elsewhere).then_some(reason)
}

/// The weekly capacity line, from structured windows. `None` when nothing was
/// reported, so the chooser can say why instead of implying spare capacity.
fn weekly_line(
    windows: &[crate::account_registry::UsageWindow],
    now: chrono::DateTime<chrono::Utc>,
) -> Option<String> {
    if windows.is_empty() {
        return None;
    }
    let Some(weekly) = windows.iter().find(|window| window.name == "weekly") else {
        return Some("weekly usage unknown (not reported)".to_string());
    };
    let reset = crate::cli::limits::format_reset(weekly.resets_at, now);
    if weekly.resets_at.is_some_and(|at| at <= now) {
        return Some(format!("weekly {reset}"));
    }
    Some(match weekly.used_percent {
        Some(used) if used.is_finite() && used >= 100.0 => format!("weekly limit hit, {reset}"),
        Some(used) if used.is_finite() && used >= 0.0 => {
            format!("weekly {:.0}% left, {reset}", 100.0 - used)
        }
        // No percentage and no reset is not evidence of an unused window.
        None if weekly.resets_at.is_none() => "weekly usage unknown (not reported)".to_string(),
        _ => format!("weekly usage unknown, {reset}"),
    })
}

fn weekly_detail(profile: &Profile) -> String {
    profile.weekly.clone().unwrap_or_else(|| {
        match profile.login_state {
            LoginState::LoggedIn => "weekly usage unknown (no recent report)",
            LoginState::Unknown => "weekly usage unknown (login unknown)",
            _ => "weekly usage unknown until login",
        }
        .to_string()
    })
}

fn choice_label(profile: &Profile, style: PickerStyle) -> String {
    let mut tags = Vec::new();
    if profile.recommendation.is_some() {
        tags.push(format!("{} recommended", style.mark(Mark::Recommended)));
    }
    if let Some(reason) = limit_tag(profile) {
        tags.push(format!("{} {reason}", style.mark(Mark::Limit)));
    }
    match profile.login_state {
        LoginState::LoggedIn => {}
        LoginState::Unknown => tags.push("login unknown".to_string()),
        _ => tags.push(format!("{} not logged in", style.mark(Mark::Limit))),
    }
    if profile.active {
        tags.push("active".to_string());
    }
    if profile.disabled {
        tags.push("disabled".to_string());
    }
    let tags: String = tags.iter().map(|tag| format!("  {tag}")).collect();
    let alias = if profile.display_name == profile.name {
        String::new()
    } else {
        format!(" ({})", shorten_picker_text(&profile.name, 24))
    };
    let display = shorten_picker_text(
        &profile.display_name,
        ROW_WIDTH.saturating_sub(text_width(&alias) + text_width(&strip_sgr(&tags))),
    );
    format!(
        "{display}{alias}{tags}\n    {}",
        shorten_picker_text(&weekly_detail(profile), ROW_WIDTH - 4)
    )
}

/// Build the chooser rows: each offered account, then the new-login entry.
///
/// Every row names one account and its state, then one weekly line. 5-hour
/// windows appear only as the limit that excluded an account; full detail is
/// `--list-profiles`.
pub(crate) fn profile_choices(profiles: &[Profile], style: PickerStyle) -> Vec<ProfileChoice> {
    let mut choices: Vec<ProfileChoice> = chooser_profiles(profiles)
        .into_iter()
        .map(|profile| ProfileChoice::Existing {
            name: profile.name.clone(),
            title: profile.display_name.clone(),
            label: choice_label(profile, style),
        })
        .collect();
    choices.push(ProfileChoice::NewLogin);
    choices
}

fn strip_sgr(value: &str) -> String {
    let mut plain = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            plain.push(c);
        }
    }
    plain
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

/// Chooser index for the starting cursor: the recommended account, else the
/// active one (or the account it duplicates), else the first row.
pub(crate) fn active_choice_index(profiles: &[Profile]) -> usize {
    let shown = chooser_profiles(profiles);
    let active = profiles.iter().find(|profile| profile.active);
    shown
        .iter()
        .position(|profile| profile.recommendation.is_some())
        .or_else(|| {
            let active = active?;
            let canonical = active.duplicate_of.as_ref().unwrap_or(&active.directory);
            shown.iter().position(|profile| &profile.directory == canonical)
        })
        .unwrap_or(0)
}

/// Show the whole list when it plausibly fits.
///
/// inquire's default page is 7. This operator has 7 accounts, which pushed
/// "log in a new account" below the fold — an option you must scroll to find
/// is an option most people never discover.
pub(crate) fn picker_page_size(choice_count: usize, option_lines: usize, terminal_rows: usize) -> usize {
    let terminal_rows = if terminal_rows == 0 { 24 } else { terminal_rows };
    choice_count.clamp(7, 20).min(terminal_rows.saturating_sub(4) / option_lines.max(1)).max(1)
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
    let style = PickerStyle::detect();
    let render = render_config(style);
    let choices = profile_choices(profiles, style);
    let starting_cursor = active_choice_index(profiles);
    let option_lines = choices.iter().map(|choice| choice.to_string().lines().count()).max().unwrap_or(1);
    let terminal_rows = crossterm::terminal::size().map(|(_, rows)| usize::from(rows)).unwrap_or(24);
    let page_size = picker_page_size(choices.len(), option_lines, terminal_rows);

    let selection = inquire::Select::new(&format!("Choose {provider_label} account"), choices)
        .with_render_config(render)
        .with_starting_cursor(starting_cursor)
        .with_page_size(page_size)
        // Repeat the account, not its whole two-line row, after Enter.
        .with_formatter(&|option| match option.value {
            ProfileChoice::Existing { title, .. } => title.clone(),
            ProfileChoice::NewLogin => "new account".to_string(),
        })
        .with_help_message(&format!(
            "Enter: this session only. Esc: cancel. Details: cas {} --list-profiles",
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
                    .with_render_config(render)
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
            stash_typeahead(provider_label)?;
            Ok(Selection::NewLogin(entered.trim().to_string()))
        }
    }
}

/// Keystrokes typed after the email prompt's Enter, for the login flow's
/// later question (the duplicate-account confirmation).
static TYPEAHEAD: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
/// A typed-ahead Ctrl-D: ends the line it follows, as the terminal would.
const TYPED_EOF: char = '\u{4}';

/// Hand the email prompt's type-ahead over to the terminal's own line
/// discipline.
///
/// inquire reads through crossterm 0.25, which reads up to 1 KiB at once and
/// keeps every key parsed after Enter in a private queue that a later
/// `std::io::stdin()` read never sees. The vendored
/// `take_buffered_events` returns exactly those already-read keys, oldest
/// first, without reading the terminal, so newer input stays queued in the
/// kernel behind them. Only finished records carry over: a line ended by
/// Enter or Ctrl-J, Ctrl-D (EOF) and Ctrl-C, which cancels now as it would
/// have when typed. Ctrl-U and Backspace edit the queued line; its unfinished,
/// never-displayed tail is dropped.
fn stash_typeahead(provider_label: &str) -> Result<()> {
    use crossterm_inquire::event::Event;
    let mut queued = String::new();
    for event in crossterm_inquire::event::take_buffered_events() {
        let Event::Key(key) = event else { continue };
        if queue_key(&mut queued, key).is_break() {
            // Nothing is staged yet; die by SIGINT like an ordinary Ctrl-C.
            #[cfg(unix)]
            unsafe {
                libc::raise(libc::SIGINT);
            }
            anyhow::bail!("new {provider_label} login cancelled");
        }
    }
    queued.truncate(queued.rfind(['\n', TYPED_EOF]).map_or(0, |end| end + 1));
    TYPEAHEAD.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push_str(&queued);
    Ok(())
}

/// Read the duplicate-account answer from input the picker did not buffer.
///
/// A terminal in canonical mode returns one record per read: a line ending in
/// a newline, or the text before a Ctrl-D, or nothing for Ctrl-D on an empty
/// line. Ctrl-D ends the answer unsent, so a record without a newline is EOF
/// (`None`); waiting for more would make the reader press Ctrl-D twice. A
/// canonical line never exceeds the kernel's line limit, far below the
/// buffer. Pipes and files are streams, not records: a short chunk is not EOF,
/// so they keep `read_line` semantics and only an empty read is EOF.
pub(crate) fn read_answer(input: &mut impl std::io::BufRead, terminal: bool) -> std::io::Result<Option<String>> {
    if !terminal {
        let mut line = String::new();
        return Ok((input.read_line(&mut line)? > 0).then_some(line));
    }
    let record = loop {
        match input.fill_buf() {
            Ok(record) => break record,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    };
    let (line, used) = match record.iter().position(|&byte| byte == b'\n') {
        Some(end) => (Some(String::from_utf8_lossy(&record[..=end]).into_owned()), end + 1),
        None => (None, record.len()),
    };
    input.consume(used);
    Ok(line)
}

/// Apply one queued key with the terminal line discipline's meaning.
fn queue_key(
    typed: &mut String,
    key: crossterm_inquire::event::KeyEvent,
) -> std::ops::ControlFlow<()> {
    use crossterm_inquire::event::{KeyCode, KeyEventKind, KeyModifiers};
    if key.kind == KeyEventKind::Release {
        return std::ops::ControlFlow::Continue(());
    }
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('c') if control => return std::ops::ControlFlow::Break(()),
        KeyCode::Char('d') if control => typed.push(TYPED_EOF),
        KeyCode::Enter => typed.push('\n'),
        KeyCode::Char('j') if control => typed.push('\n'),
        // Ctrl-U erases the unfinished line, never a finished record.
        KeyCode::Char('u') if control => {
            typed.truncate(typed.rfind(['\n', TYPED_EOF]).map_or(0, |end| end + 1))
        }
        KeyCode::Backspace if !typed.is_empty() && !typed.ends_with(['\n', TYPED_EOF]) => {
            typed.pop();
        }
        KeyCode::Char(c) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
            typed.push(c)
        }
        _ => {}
    }
    std::ops::ControlFlow::Continue(())
}

/// The next finished line typed ahead of the question that asks for it, as
/// `read_line` returns it: `y\n`, or `""` for Ctrl-D (EOF). An unfinished
/// fragment is discarded: it was read after the email's Enter and never shown,
/// and from here the terminal's own line discipline owns all input.
pub(crate) fn take_typeahead_line() -> Option<String> {
    next_typed_line(&mut TYPEAHEAD.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))
}

fn next_typed_line(typed: &mut String) -> Option<String> {
    let Some(end) = typed.find(['\n', TYPED_EOF]) else {
        typed.clear();
        return None;
    };
    let line: String = typed.drain(..=end).collect();
    Some(if line.ends_with(TYPED_EOF) { String::new() } else { line })
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
        // Only a marker segment after a dot is scratch; a whole alias such as
        // `old` names a real account (cas-deb5). Login staging stays hidden.
        for (name, account) in [
            ("support@petrastella.io", true),
            ("alt", true),
            ("old", true),
            ("backup", true),
            ("", false),
            ("support@petrastella.io.lock", false),
            ("work.bak", false),
            ("work.tmp.1777306474", false),
            ("old-profile.old", false),
            ("login.tmp.AbC123", false),
        ] {
            assert_eq!(is_account_profile_name(name), account, "{name:?}");
        }
        for layout in [CLAUDE, CODEX] {
            let home = TempDir::new().unwrap();
            for name in ["old", "backup", "login.tmp.AbC123", "work.old"] {
                std::fs::create_dir(home.path().join(format!("{}{name}", layout.named_prefix)))
                    .unwrap();
            }
            let profiles =
                scan_profiles_with(layout, home.path(), None, |_, _| LoginState::Unknown).unwrap();
            let names: Vec<&str> = profiles.iter().map(|p| p.name.as_str()).collect();
            assert_eq!(names, ["main", "backup", "old"], "{}", layout.named_prefix);
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
        std::fs::create_dir(home.path().join(".codex-operator@example.com")).unwrap();
        std::fs::copy(alt.join("auth.json"), home.path().join(".codex-operator@example.com/auth.json")).unwrap();
        // A never-used folder (no credential, email or identity; logged out).
        std::fs::create_dir(home.path().join(".codex-grok")).unwrap();
        let profiles = scan_profiles_with(CODEX, home.path(), Some(alt.as_path()), |name, _| {
            match name {
                "alt" => LoginState::LoggedIn,
                "grok" => LoginState::LoggedOut,
                _ => LoginState::Unknown,
            }
        })
        .unwrap();

        let names: Vec<&str> = profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["main", "grok", "operator@example.com", "alt", "work"]);
        assert!(profiles[3].active);
        assert!(!profiles[0].active);
        // The chooser offers each identity once and hides the never-used
        // folder; the active duplicate's cursor moves to its canonical row.
        let rows: Vec<String> =
            profile_choices(&profiles, PickerStyle { glyphs: false, styled: false })
                .iter()
                .map(ToString::to_string)
                .collect();
        assert_eq!(
            rows,
            [
                "main  login unknown\n    weekly usage unknown (login unknown)",
                "operator@example.com  login unknown\n    weekly usage unknown (login unknown)",
                "+ Log in a new account",
            ]
        );
        assert_eq!(active_choice_index(&profiles), 1);
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
                weekly: None,
                duplicate_of: None,
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
                weekly: None,
                duplicate_of: None,
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
                weekly: None,
                duplicate_of: None,
                recommendation: None,
                exclusion: Some("not logged in or login unknown".into()),
            },
            Profile {
                name: "alt".to_string(),
                display_name: "alt".to_string(),
                directory: PathBuf::from("/tmp/.codex-alt"),
                login_state: LoginState::Unknown,
                active: false,
                disabled: true,
                usage: None,
                weekly: None,
                duplicate_of: None,
                recommendation: None,
                exclusion: None,
            },
        ];

        const PLAIN: PickerStyle = PickerStyle { glyphs: false, styled: false };
        let text = |profiles: &[Profile], style| -> Vec<String> {
            profile_choices(profiles, style).iter().map(ToString::to_string).collect()
        };
        // Unknown capacity says why; it is never shown as spare capacity.
        assert_eq!(
            text(&profiles, PLAIN),
            [
                "main  [WARN] not logged in  active\n    weekly usage unknown until login",
                "alt  login unknown  disabled\n    weekly usage unknown (login unknown)",
                "+ Log in a new account",
            ]
        );
        assert_eq!(active_choice_index(&profiles), 0);
        let now = "2026-10-27T12:00:00Z".parse::<chrono::DateTime<chrono::Utc>>().unwrap();
        let reset = |hours| crate::cli::limits::format_reset(Some(now + chrono::Duration::hours(hours)), now);
        for provider in crate::accounts::Provider::ALL {
            let mut accounts = vec![
                crate::account_selection::tests::row(provider, "main", 48, 0.0, 0.0),
                crate::account_selection::tests::row(provider, "alt", 6, 59.0, 20.0),
            ];
            for account in &mut accounts {
                account.windows.push(crate::account_registry::UsageWindow { name: "extra usage".into(), used_percent: None, resets_at: None });
            }
            let mut profiles = profiles.clone();
            for (profile, account) in profiles.iter_mut().zip(&accounts) { profile.directory = account.folder.clone(); }
            apply_recommendation(&mut profiles, &accounts, provider, now, &Default::default());
            // Weekly capacity is the first detail; the policy's choice is the
            // marked, preselected row. Only the glyph carries colour.
            let rows = text(&profiles, PickerStyle { glyphs: true, styled: true });
            assert_eq!(
                rows[..2],
                [
                    format!("main  active\n    weekly 100% left, {}", reset(48)),
                    format!("alt  \x1b[38;2;40;145;75m\u{2713}\x1b[39m recommended  disabled\n    weekly 41% left, {}", reset(6)),
                ]
            );
            assert_eq!(active_choice_index(&profiles), 1);
            assert!(rows.iter().all(|row| !row.contains("5h") && !row.contains("extra usage")));
            assert!(rows.iter().all(|row| row.lines().all(|line| text_width(&strip_sgr(line)) <= ROW_WIDTH)));
            // A 5-hour window appears only as the limit that excluded a row.
            accounts[1].windows[0].used_percent = Some(95.0);
            accounts[1].windows[1].used_percent = None;
            accounts[1].windows[1].resets_at = None;
            accounts[0].windows.clear();
            apply_recommendation(&mut profiles, &accounts, provider, now, &Default::default());
            assert_eq!(
                text(&profiles, PLAIN)[..2],
                [
                    "main  active\n    weekly usage unknown (no recent report)".to_string(),
                    "alt  [WARN] 5h at 95% (threshold 90%)  disabled\n    weekly usage unknown (not reported)".to_string(),
                ]
            );
        }
    }

    #[test]
    fn queued_keys_follow_the_terminal_line_discipline() {
        use crossterm_inquire::event::{KeyCode, KeyEvent, KeyModifiers};
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        let mut typed = String::new();
        for queued in [
            key(KeyCode::Char('n')),
            key(KeyCode::Backspace),
            key(KeyCode::Char('y')),
            key(KeyCode::Enter),
            ctrl('d'),
            key(KeyCode::Char('n')),
        ] {
            assert!(queue_key(&mut typed, queued).is_continue());
        }
        assert_eq!(next_typed_line(&mut typed).as_deref(), Some("y\n"));
        // Ctrl-D is EOF, which cancels like stdin EOF.
        assert_eq!(next_typed_line(&mut typed).as_deref(), Some(""));
        // An unfinished, undisplayed fragment does not carry over.
        assert_eq!(next_typed_line(&mut typed), None);
        assert!(typed.is_empty());
        for queued in [key(KeyCode::Char('y')), ctrl('j'), key(KeyCode::Char('y')), ctrl('u'), key(KeyCode::Enter)] {
            assert!(queue_key(&mut typed, queued).is_continue());
        }
        assert_eq!(next_typed_line(&mut typed).as_deref(), Some("y\n"));
        // Ctrl-U erased the second `y` before its Enter: an empty answer.
        assert_eq!(next_typed_line(&mut typed).as_deref(), Some("\n"));
        assert!(queue_key(&mut typed, ctrl('c')).is_break());
    }

    #[cfg(unix)]
    #[test]
    fn answers_are_terminal_records_but_pipe_streams() {
        use std::io::{BufReader, Read, Write};
        use std::os::fd::FromRawFd;
        // A real terminal pair in its default canonical mode.
        let (mut master, mut slave) = (0, 0);
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            },
            0
        );
        let mut keyboard = unsafe { std::fs::File::from_raw_fd(master) };
        let mut terminal = BufReader::new(unsafe { std::fs::File::from_raw_fd(slave) });
        let mut type_keys = |keys: &[u8]| {
            keyboard.write_all(keys).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(50));
        };
        // Erase is the terminal's; each record is read once, in order.
        type_keys(b"nx\x7f\x7fy\rn\r");
        assert_eq!(read_answer(&mut terminal, true).unwrap().as_deref(), Some("y\n"));
        assert_eq!(read_answer(&mut terminal, true).unwrap().as_deref(), Some("n\n"));
        // Ctrl-D after an unfinished answer, and on an empty line, is EOF.
        type_keys(b"y\x04");
        assert_eq!(read_answer(&mut terminal, true).unwrap(), None);
        type_keys(b"\x04");
        assert_eq!(read_answer(&mut terminal, true).unwrap(), None);
        // A pipe chunk without a newline is not EOF: the stream continues.
        let stream = std::io::Cursor::new(b"y".to_vec()).chain(std::io::Cursor::new(b"es\n".to_vec()));
        let mut pipe = BufReader::with_capacity(1, stream);
        assert_eq!(read_answer(&mut pipe, false).unwrap().as_deref(), Some("yes\n"));
        assert_eq!(read_answer(&mut pipe, false).unwrap(), None);
    }

    #[test]
    fn the_new_login_row_is_never_hidden_below_the_fold() {
        // 7 accounts + the new-login row must all be visible at once.
        assert_eq!(picker_page_size(8, 1, 24), 8);
        assert_eq!(picker_page_size(3, 1, 24), 7);
        assert_eq!(picker_page_size(50, 1, 24), 20);
        assert_eq!(picker_page_size(8, 4, 24), 5);
        assert_eq!(picker_page_size(8, 4, 0), 5);
        assert_eq!(picker_page_size(8, 4, 10), 1);
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
