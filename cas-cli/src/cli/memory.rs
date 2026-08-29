//! `cas memory` CLI — retroactive team-share backfill.
//!
//! Provides `share`/`unshare` operations that mutate `Entry.share` and rely
//! on the `SyncingEntryStore` wrapper to dual-enqueue the resulting write
//! into the active team's push queue. The predicate used for `--since` and
//! `--all` is `share_policy::eligible_for_team_entry`, keeping selection
//! aligned with the T1 auto-promote path.

use std::path::Path;

use anyhow::{Context, anyhow};
use cas_types::{Entry, EntryType, Scope, ShareScope};
use clap::{Parser, Subcommand};

use crate::cli::Cli;
use crate::cloud::CloudConfig;
use crate::store::{open_store, open_store_local};

#[derive(Subcommand)]
pub enum MemoryCommands {
    /// Report stored memories containing tool-call/XML artifacts (read-only)
    Hygiene,
    /// Share personal memories with your team (retroactive backfill)
    ///
    /// Sets `share = Team` on selected entries and enqueues them for
    /// the next `cas cloud sync`. Requires a team to be configured
    /// via `cas cloud team set <uuid>`; without one, entries are
    /// marked on disk but no team-queue rows are written.
    Share(ShareArgs),
    /// Unshare a memory from the team (sets `share = Private`)
    ///
    /// Blocks future team dual-enqueue for this entry. Note: this
    /// only affects local promotion — entries already synced to the
    /// cloud team store are not retracted by this command.
    Unshare(UnshareArgs),
}

#[derive(Parser)]
pub struct ShareArgs {
    /// Entry id to promote (e.g., 2026-03-01-1).
    ///
    /// Mutually exclusive with --since and --all. Fails if the entry
    /// is a Preference or Global-scoped (stay-personal by default).
    #[arg(conflicts_with_all = ["since", "all"])]
    pub id: Option<String>,

    /// Promote all entries created within the given duration.
    ///
    /// Examples: 7d, 48h, 30m, 90s, 2w. Preference-typed and
    /// Global-scoped entries are skipped automatically.
    #[arg(long, conflicts_with_all = ["id", "all"])]
    pub since: Option<String>,

    /// Promote every eligible entry in the store.
    ///
    /// Eligible = Project scope, not Preference-typed, not already
    /// marked share=Private. Use this for initial team onboarding
    /// when pre-existing personal memories need to be promoted in
    /// bulk.
    #[arg(long, conflicts_with_all = ["id", "since"])]
    pub all: bool,

    /// Preview what would be promoted without mutating the store.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Parser)]
pub struct UnshareArgs {
    /// Entry id to demote back to personal-only.
    pub id: String,
}

pub fn execute(cmd: &MemoryCommands, _cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    match cmd {
        MemoryCommands::Hygiene => execute_hygiene(cas_root),
        MemoryCommands::Share(args) => execute_share(args, cas_root),
        MemoryCommands::Unshare(args) => execute_unshare(args, cas_root),
    }
}

/// Scan stored memories and report likely tool-call/XML contamination.
///
/// This command is intentionally report-only. It never rewrites or deletes an
/// entry; operators decide how a finding should be corrected.
pub fn execute_hygiene(cas_root: &Path) -> anyhow::Result<()> {
    let store = open_store_local(cas_root).context("failed to open entry store")?;
    let entries = store.list()?;
    let findings = cas_core::memory::find_contaminated_entries(&entries);
    if findings.is_empty() {
        println!("memory hygiene: no tool-call/XML artifacts found");
        return Ok(());
    }

    println!(
        "memory hygiene: {} contaminated entr{} found (report only)",
        findings.len(),
        if findings.len() == 1 { "y" } else { "ies" }
    );
    for finding in findings {
        println!("  {}: {}", finding.id, finding.patterns.join(", "));
    }
    Ok(())
}

/// Public for integration tests only. Not a stable API.
#[doc(hidden)]
pub fn execute_share(args: &ShareArgs, cas_root: &Path) -> anyhow::Result<()> {
    // Exactly one of id | --since | --all must be supplied. clap's
    // `conflicts_with_all` prevents combinations; check the empty case here.
    if args.id.is_none() && args.since.is_none() && !args.all {
        return Err(anyhow!(
            "Specify one of <id>, --since <duration>, or --all. See `cas memory share --help`."
        ));
    }

    // Warn (but don't block) when no team is configured — the user can still
    // mark entries as share=Team; dual-enqueue will be a no-op until a team
    // is set. A silent success would hide the "nothing happened" outcome.
    let cloud_cfg = CloudConfig::load_from_cas_dir(cas_root).unwrap_or_default();
    if cloud_cfg.active_team_id().is_none() {
        eprintln!(
            "warning: no active team configured (or team_auto_promote disabled). \
             Entries will be marked share=Team on disk but no team-queue rows will \
             be enqueued until you run `cas cloud team default <slug>` (user-wide) \
             or `cas cloud team set <uuid>` (per-project) and write again."
        );
    }

    let store = open_store(cas_root).context("failed to open entry store")?;

    // `store.list()` is LIMIT 10000 ORDER BY created DESC. Bulk modes
    // warn when the cap is hit so users don't silently miss old entries.
    const LIST_CAP: usize = 10000;

    let candidates: Vec<Entry> = if let Some(id) = args.id.as_deref() {
        vec![store
            .get(id)
            .with_context(|| format!("entry {id} not found"))?]
    } else if let Some(since) = args.since.as_deref() {
        let duration = parse_duration(since)?;
        let cutoff = chrono::Utc::now() - duration;
        let rows = store.list()?;
        if rows.len() >= LIST_CAP {
            eprintln!(
                "warning: result truncated at {LIST_CAP} entries (ORDER BY created DESC); \
                 older matches may be missed. Narrow --since or run again after sync."
            );
        }
        rows.into_iter().filter(|e| e.created >= cutoff).collect()
    } else {
        // --all
        let rows = store.list()?;
        if rows.len() >= LIST_CAP {
            eprintln!(
                "warning: result truncated at {LIST_CAP} entries (ORDER BY created DESC); \
                 older entries may be missed. Use --since to target a specific window."
            );
        }
        rows
    };

    let mut promoted = 0usize;
    let mut skipped_ineligible = 0usize;
    let mut already_shared = 0usize;

    for mut entry in candidates {
        if entry.share == Some(ShareScope::Team) {
            already_shared += 1;
            continue;
        }

        // Apply the T1 filter directly on scope/type (share is set below);
        // Private explicitly blocks promotion regardless of scope.
        let scope_type_eligible =
            entry.scope == Scope::Project && entry.entry_type != EntryType::Preference;

        if entry.share == Some(ShareScope::Private) || !scope_type_eligible {
            skipped_ineligible += 1;
            if args.id.is_some() {
                // Single-id mode: surface the reason so the user knows
                // why nothing happened. Private is terminal — there is
                // currently no CLI to clear it; flag the dead-end so
                // the user knows to file for a reset rather than retry.
                let hint = if entry.share == Some(ShareScope::Private) {
                    " (share=Private is a hard override set by `cas memory unshare`; \
                       there is no `--force` override in this release)"
                } else {
                    " (Preference-typed or Global-scoped entries stay personal by T1 policy)"
                };
                return Err(anyhow!(
                    "entry {} is not eligible for team share (share={:?}, scope={:?}, type={:?}).{hint}",
                    entry.id,
                    entry.share,
                    entry.scope,
                    entry.entry_type,
                ));
            }
            continue;
        }

        if args.dry_run {
            promoted += 1;
            continue;
        }

        entry.share = Some(ShareScope::Team);
        store
            .update(&entry)
            .with_context(|| format!("failed to update entry {}", entry.id))?;
        promoted += 1;
    }

    let label = if args.dry_run { "would promote" } else { "promoted" };
    println!(
        "{label} {promoted} entries \
         (skipped {skipped_ineligible} ineligible, \
         {already_shared} already shared)"
    );

    Ok(())
}

/// Public for integration tests only. Not a stable API.
#[doc(hidden)]
pub fn execute_unshare(args: &UnshareArgs, cas_root: &Path) -> anyhow::Result<()> {
    let store = open_store(cas_root).context("failed to open entry store")?;
    let mut entry = store
        .get(&args.id)
        .with_context(|| format!("entry {} not found", args.id))?;

    if entry.share == Some(ShareScope::Private) {
        println!("entry {} already marked share=Private (no-op)", args.id);
        return Ok(());
    }

    entry.share = Some(ShareScope::Private);
    store
        .update(&entry)
        .with_context(|| format!("failed to update entry {}", entry.id))?;
    println!("entry {} marked share=Private", args.id);
    Ok(())
}

/// Parse a duration string like `7d`, `48h`, `30m`, `90s` into a
/// `chrono::Duration`. Kept deliberately simple — humantime would pull
/// in a new dep for one call site.
fn parse_duration(s: &str) -> anyhow::Result<chrono::Duration> {
    let s = s.trim();
    if s.is_empty() {
        return Err(anyhow!("duration is empty"));
    }
    let (num, unit) = s.split_at(
        s.find(|c: char| !c.is_ascii_digit())
            .ok_or_else(|| anyhow!("duration missing unit (e.g. 7d, 48h, 30m)"))?,
    );
    let n: i64 = num
        .parse()
        .with_context(|| format!("invalid duration number: {num}"))?;
    let d = match unit {
        "s" => chrono::Duration::try_seconds(n),
        "m" => chrono::Duration::try_minutes(n),
        "h" => chrono::Duration::try_hours(n),
        "d" => chrono::Duration::try_days(n),
        "w" => chrono::Duration::try_weeks(n),
        other => return Err(anyhow!("unknown duration unit `{other}` (expected s/m/h/d/w)")),
    };
    d.ok_or_else(|| anyhow!("duration {n}{unit} overflows i64 nanoseconds"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_accepts_common_units() {
        assert_eq!(parse_duration("7d").unwrap(), chrono::Duration::days(7));
        assert_eq!(parse_duration("48h").unwrap(), chrono::Duration::hours(48));
        assert_eq!(parse_duration("30m").unwrap(), chrono::Duration::minutes(30));
        assert_eq!(parse_duration("90s").unwrap(), chrono::Duration::seconds(90));
        assert_eq!(parse_duration("2w").unwrap(), chrono::Duration::weeks(2));
    }

    #[test]
    fn parse_duration_rejects_bad_input() {
        assert!(parse_duration("").is_err());
        assert!(parse_duration("7").is_err()); // missing unit
        assert!(parse_duration("abc").is_err());
        assert!(parse_duration("7y").is_err()); // unknown unit
    }
}
