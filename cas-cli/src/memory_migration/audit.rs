//! The zero-loss audit (AC1).
//!
//! The epic's requirement is that **every** legacy row is accounted for — as
//! migrated, deliberately left, or merged into something else — with no silent
//! drops. The bucket list here is the spec's five dispositions rather than the
//! acceptance criterion's shorthand three, because `stay-entry` (rules R4/R5/R9)
//! is the bulk of the real corpus and is a first-class disposition in §4: an
//! identity that omitted it could never balance.
//!
//! `unaccounted` exists so the failure is loud. It can only be non-zero if a
//! row was extracted and then dropped between routing and counting, which is
//! precisely the silent-loss failure mode this audit is defending against.

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use serde::Serialize;

use super::routing::Disposition;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LossAudit {
    pub total: usize,
    pub migrated: usize,
    pub carried_verbatim: usize,
    pub stay_entry: usize,
    pub deliberately_left: usize,
    pub merged_into: usize,
    /// MUST be 0. Any other value is a hard error, never a warning.
    pub unaccounted: usize,
    /// Row count per §4 rule — this is what makes the audit reviewable rather
    /// than merely balanced.
    pub per_rule: BTreeMap<String, usize>,
    /// Row count per source database.
    pub per_db: BTreeMap<String, usize>,
    /// Quarantined rows (a subset of `deliberately_left`, all rule R6).
    pub quarantined: usize,
}

impl LossAudit {
    pub fn record(&mut self, db: &str, rule: &str, disposition: Disposition) {
        self.total += 1;
        *self.per_rule.entry(rule.to_string()).or_insert(0) += 1;
        *self.per_db.entry(db.to_string()).or_insert(0) += 1;
        match disposition {
            Disposition::MigrateToPage => self.migrated += 1,
            Disposition::CarryVerbatim => self.carried_verbatim += 1,
            Disposition::StayEntry => self.stay_entry += 1,
            Disposition::MergedInto => self.merged_into += 1,
            Disposition::DeliberatelyLeave => self.deliberately_left += 1,
        }
        if rule == "R6" {
            self.quarantined += 1;
        }
    }

    /// A row was extracted but reached no bucket. Should be unreachable — the
    /// §4 procedure is total and an unrouted row aborts — so this is the
    /// belt-and-braces counter that turns a future regression into a failure
    /// instead of a quiet shortfall.
    pub fn record_unaccounted(&mut self, db: &str) {
        self.total += 1;
        self.unaccounted += 1;
        *self.per_db.entry(db.to_string()).or_insert(0) += 1;
    }

    fn accounted(&self) -> usize {
        self.migrated
            + self.carried_verbatim
            + self.stay_entry
            + self.deliberately_left
            + self.merged_into
    }

    pub fn balances(&self) -> bool {
        self.unaccounted == 0 && self.accounted() == self.total
    }

    /// AC1's hard error.
    pub fn check(&self) -> Result<()> {
        if self.unaccounted != 0 {
            bail!(
                "loss audit: {} legacy row(s) are unaccounted for — migration aborts \
                 rather than drop them silently",
                self.unaccounted
            );
        }
        if self.accounted() != self.total {
            bail!(
                "loss audit does not balance: {} accounted vs {} extracted",
                self.accounted(),
                self.total
            );
        }
        Ok(())
    }

    pub fn render_table(&self) -> String {
        let mut out = String::new();
        out.push_str("legacy row loss audit\n");
        out.push_str("─────────────────────────────────────────────\n");
        let rows = [
            ("migrated (page)", self.migrated),
            ("carried-verbatim (locked page)", self.carried_verbatim),
            ("stay-entry", self.stay_entry),
            ("deliberately-left", self.deliberately_left),
            ("  of which quarantined (R6)", self.quarantined),
            ("merged-into", self.merged_into),
            ("unaccounted", self.unaccounted),
        ];
        for (label, count) in rows {
            out.push_str(&format!("{label:<34}{count:>8}\n"));
        }
        out.push_str("─────────────────────────────────────────────\n");
        out.push_str(&format!(
            "{:<34}{:>8}\n",
            "legacy rows extracted", self.total
        ));
        out.push_str(&format!("{:<34}{:>8}\n", "accounted for", self.accounted()));
        out.push_str(&format!(
            "balance: {}\n",
            if self.balances() {
                "OK — zero loss"
            } else {
                "FAILED"
            }
        ));

        out.push_str("\nby rule (§4 decision procedure)\n");
        for (rule, count) in &self.per_rule {
            out.push_str(&format!("  {rule:<6}{count:>8}\n"));
        }
        out.push_str("\nby source database\n");
        for (db, count) in &self.per_db {
            out.push_str(&format!("  {db:<10}{count:>8}\n"));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_balanced_audit_passes_and_reports_its_rules() {
        let mut audit = LossAudit::default();
        audit.record("project", "R1", Disposition::DeliberatelyLeave);
        audit.record("project", "R7", Disposition::MigrateToPage);
        audit.record("global", "R3", Disposition::CarryVerbatim);
        assert!(audit.balances());
        audit.check().unwrap();
        let table = audit.render_table();
        assert!(table.contains("zero loss"));
        assert!(table.contains("R7"));
        assert!(table.contains("global"));
    }

    #[test]
    fn an_unaccounted_row_is_a_hard_error_not_a_warning() {
        let mut audit = LossAudit::default();
        audit.record_unaccounted("project");
        assert!(!audit.balances());
        let err = audit.check().unwrap_err().to_string();
        assert!(err.contains("unaccounted"), "{err}");
        assert!(audit.render_table().contains("FAILED"));
    }

    #[test]
    fn quarantined_rows_are_counted_inside_deliberately_left() {
        let mut audit = LossAudit::default();
        audit.record("project", "R6", Disposition::DeliberatelyLeave);
        assert_eq!(audit.quarantined, 1);
        assert_eq!(audit.deliberately_left, 1);
        // Not double-counted into the total.
        assert_eq!(audit.total, 1);
        assert!(audit.balances());
    }
}
