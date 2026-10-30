//! Temporal knowledge graph queries
//!
//! Enables time-based queries on the knowledge graph:
//! - Point-in-time entity state
//! - Relationship history
//! - Temporal filtering for searches
//! - Temporal retrieval as a search channel (Hindsight-inspired)
//!
//! # Time-aware Features
//!
//! - Relationships have `valid_from` and `valid_until` timestamps
//! - Entities track creation/update times
//! - Entries have valid_from/valid_until for temporal validity
//! - Queries can filter by time period
//! - TemporalRetriever provides entries valid at a given time

use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};

use cas_types::Relationship;

/// Error type for temporal query parsing
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemporalParseError {
    /// The input was empty or whitespace-only
    EmptyInput,
    /// The pattern was recognized but had invalid values
    InvalidDate { pattern: String, reason: String },
    /// The start date was after the end date in a range
    InvalidRange { start: String, end: String },
    /// The pattern was not recognized
    UnrecognizedPattern { input: String },
}

impl std::fmt::Display for TemporalParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyInput => write!(f, "empty input"),
            Self::InvalidDate { pattern, reason } => {
                write!(f, "invalid date in '{pattern}': {reason}")
            }
            Self::InvalidRange { start, end } => {
                write!(f, "invalid range: start '{start}' is after end '{end}'")
            }
            Self::UnrecognizedPattern { input } => {
                write!(f, "unrecognized temporal pattern: '{input}'")
            }
        }
    }
}

impl std::error::Error for TemporalParseError {}

/// A time period for temporal queries
#[derive(Debug, Clone)]
pub struct TimePeriod {
    /// Start of the period (inclusive)
    pub start: DateTime<Utc>,
    /// End of the period (inclusive)
    pub end: DateTime<Utc>,
}

impl TimePeriod {
    /// Create a period from two dates
    pub fn new(start: DateTime<Utc>, end: DateTime<Utc>) -> Self {
        Self { start, end }
    }

    /// Create a period for a specific day
    pub fn day(date: NaiveDate) -> Self {
        let start = date.and_hms_opt(0, 0, 0).unwrap().and_utc();
        let end = date
            .and_hms_nano_opt(23, 59, 59, 999_999_999)
            .unwrap()
            .and_utc();
        Self { start, end }
    }

    /// Create a period for a specific month
    pub fn month(year: i32, month: u32) -> Option<Self> {
        let start_date = NaiveDate::from_ymd_opt(year, month, 1)?;
        let end_date = if month == 12 {
            NaiveDate::from_ymd_opt(year + 1, 1, 1)?.pred_opt()?
        } else {
            NaiveDate::from_ymd_opt(year, month + 1, 1)?.pred_opt()?
        };

        Some(Self {
            start: start_date.and_hms_opt(0, 0, 0)?.and_utc(),
            end: end_date.and_hms_opt(23, 59, 59)?.and_utc(),
        })
    }

    /// Create a period for the last N days
    pub fn last_days(days: i64) -> Self {
        Self::last_days_at(days, Utc::now())
    }

    fn last_days_at(days: i64, end: DateTime<Utc>) -> Self {
        let start = end - Duration::days(days);
        Self { start, end }
    }

    /// Create a period for the last N weeks
    pub fn last_weeks(weeks: i64) -> Self {
        Self::last_days(weeks * 7)
    }

    /// Create a period for the last N months (approximate)
    pub fn last_months(months: i64) -> Self {
        Self::last_days(months * 30)
    }

    /// Check if a timestamp falls within this period
    pub fn contains(&self, timestamp: &DateTime<Utc>) -> bool {
        *timestamp >= self.start && *timestamp <= self.end
    }

    /// Check if a relationship was valid during any part of this period
    pub fn overlaps_relationship(&self, rel: &Relationship) -> bool {
        // If no validity bounds, relationship is always valid
        let rel_start = rel
            .valid_from
            .unwrap_or(DateTime::UNIX_EPOCH.with_timezone(&Utc));
        let rel_end = rel
            .valid_until
            .unwrap_or_else(|| Utc::now() + Duration::days(36500));

        // Check for overlap
        self.start <= rel_end && self.end >= rel_start
    }
}

/// Parse a date string with flexible format support
///
/// Supports:
/// - ISO format: "2025-01-15"
/// - US format: "01/15/2025", "1/15/2025"
/// - EU format: "15-01-2025", "15.01.2025"
/// - Natural: "January 15, 2025", "Jan 15, 2025", "15 January 2025"
pub fn parse_date_flexible(text: &str) -> Option<NaiveDate> {
    let text = text.trim();

    // ISO format: YYYY-MM-DD
    if let Ok(date) = NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        return Some(date);
    }

    // US format: MM/DD/YYYY or M/D/YYYY
    if text.contains('/') {
        let parts: Vec<&str> = text.split('/').collect();
        if parts.len() == 3 {
            if let (Ok(month), Ok(day), Ok(year)) = (
                parts[0].parse::<u32>(),
                parts[1].parse::<u32>(),
                parts[2].parse::<i32>(),
            ) {
                // Only accept if year is 4 digits (to avoid MM/DD/YY ambiguity)
                if (1900..=2100).contains(&year)
                    && (1..=12).contains(&month)
                    && (1..=31).contains(&day)
                {
                    return NaiveDate::from_ymd_opt(year, month, day);
                }
            }
        }
    }

    // EU format with dashes: DD-MM-YYYY
    if text.matches('-').count() == 2
        && !text.starts_with(|c: char| c.is_ascii_digit() && text.len() >= 10 && &text[4..5] == "-")
    {
        let parts: Vec<&str> = text.split('-').collect();
        if parts.len() == 3 {
            if let (Ok(day), Ok(month), Ok(year)) = (
                parts[0].parse::<u32>(),
                parts[1].parse::<u32>(),
                parts[2].parse::<i32>(),
            ) {
                if (1900..=2100).contains(&year)
                    && (1..=12).contains(&month)
                    && (1..=31).contains(&day)
                {
                    return NaiveDate::from_ymd_opt(year, month, day);
                }
            }
        }
    }

    // EU format with dots: DD.MM.YYYY
    if text.contains('.') {
        let parts: Vec<&str> = text.split('.').collect();
        if parts.len() == 3 {
            if let (Ok(day), Ok(month), Ok(year)) = (
                parts[0].parse::<u32>(),
                parts[1].parse::<u32>(),
                parts[2].parse::<i32>(),
            ) {
                if (1900..=2100).contains(&year)
                    && (1..=12).contains(&month)
                    && (1..=31).contains(&day)
                {
                    return NaiveDate::from_ymd_opt(year, month, day);
                }
            }
        }
    }

    // Natural format: "January 15, 2025" or "Jan 15, 2025"
    let months = [
        ("january", 1),
        ("jan", 1),
        ("february", 2),
        ("feb", 2),
        ("march", 3),
        ("mar", 3),
        ("april", 4),
        ("apr", 4),
        ("may", 5),
        ("june", 6),
        ("jun", 6),
        ("july", 7),
        ("jul", 7),
        ("august", 8),
        ("aug", 8),
        ("september", 9),
        ("sep", 9),
        ("sept", 9),
        ("october", 10),
        ("oct", 10),
        ("november", 11),
        ("nov", 11),
        ("december", 12),
        ("dec", 12),
    ];

    let text_lower = text.to_lowercase();

    for (month_name, month_num) in months {
        // "January 15, 2025" or "January 15 2025"
        if text_lower.starts_with(month_name) {
            let rest = text_lower.strip_prefix(month_name)?.trim();
            // Remove comma if present
            let rest = rest.replace(',', " ");
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.len() == 2 {
                if let (Ok(day), Ok(year)) = (parts[0].parse::<u32>(), parts[1].parse::<i32>()) {
                    if (1900..=2100).contains(&year) && (1..=31).contains(&day) {
                        return NaiveDate::from_ymd_opt(year, month_num, day);
                    }
                }
            }
        }

        // "15 January 2025"
        let pattern = format!(" {month_name} ");
        if text_lower.contains(&pattern) || text_lower.contains(&format!(" {month_name}")) {
            let parts: Vec<&str> = text_lower.split_whitespace().collect();
            if parts.len() == 3 {
                // Find month position
                let month_idx = parts
                    .iter()
                    .position(|&p| p == month_name || p.starts_with(month_name))?;
                if month_idx == 1 {
                    // Day Month Year
                    if let (Ok(day), Ok(year)) = (parts[0].parse::<u32>(), parts[2].parse::<i32>())
                    {
                        if (1900..=2100).contains(&year) && (1..=31).contains(&day) {
                            return NaiveDate::from_ymd_opt(year, month_num, day);
                        }
                    }
                }
            }
        }
    }

    None
}

/// Parsed temporal query
#[derive(Debug, Clone)]
pub struct TemporalQuery {
    /// The time period to query
    pub period: TimePeriod,

    /// Natural language description of the period
    pub description: String,
}

impl TemporalQuery {
    /// Parse a temporal query from natural language
    ///
    /// Supported formats:
    /// - "in January 2025" / "January 2025"
    /// - "last week" / "last month" / "last year"
    /// - "last 7 days" / "last 30 days"
    /// - "today" / "yesterday"
    /// - "2025-01-15" (ISO date)
    /// - "since 2025-01-01"
    /// - "between 2025-01-01 and 2025-01-31"
    pub fn parse(text: &str) -> Option<Self> {
        Self::parse_at(text, Utc::now())
    }

    /// Parse relative periods against one caller-supplied clock snapshot.
    fn parse_at(text: &str, now: DateTime<Utc>) -> Option<Self> {
        let text = text.trim().to_lowercase();

        // "today"
        if text == "today" {
            return Some(Self {
                period: TimePeriod::day(now.date_naive()),
                description: "today".to_string(),
            });
        }

        // "yesterday"
        if text == "yesterday" {
            let yesterday = now.date_naive().pred_opt()?;
            return Some(Self {
                period: TimePeriod::day(yesterday),
                description: "yesterday".to_string(),
            });
        }

        // "last week"
        if text == "last week" {
            return Some(Self {
                period: TimePeriod::last_days_at(7, now),
                description: "last week".to_string(),
            });
        }

        // "last month"
        if text == "last month" {
            return Some(Self {
                period: TimePeriod::last_days_at(30, now),
                description: "last month".to_string(),
            });
        }

        // "last year"
        if text == "last year" {
            return Some(Self {
                period: TimePeriod::last_days_at(365, now),
                description: "last year".to_string(),
            });
        }

        // "recently" - last 7 days
        if text == "recently" {
            return Some(Self {
                period: TimePeriod::last_days_at(7, now),
                description: "recently".to_string(),
            });
        }

        // "this week" - from start of current week to now
        if text == "this week" {
            let weekday = now.weekday().num_days_from_monday() as i64;
            let start = now - Duration::days(weekday);
            let start = start.date_naive().and_hms_opt(0, 0, 0)?.and_utc();
            return Some(Self {
                period: TimePeriod::new(start, now),
                description: "this week".to_string(),
            });
        }

        // "this month" - from start of current month to now
        if text == "this month" {
            let start = now
                .date_naive()
                .with_day(1)?
                .and_hms_opt(0, 0, 0)?
                .and_utc();
            return Some(Self {
                period: TimePeriod::new(start, now),
                description: "this month".to_string(),
            });
        }

        // "this year" - from start of current year to now
        if text == "this year" {
            let start_date = NaiveDate::from_ymd_opt(now.date_naive().year(), 1, 1)?;
            let start = start_date.and_hms_opt(0, 0, 0)?.and_utc();
            return Some(Self {
                period: TimePeriod::new(start, now),
                description: "this year".to_string(),
            });
        }

        // "N days ago" / "N weeks ago" / "N months ago"
        if text.ends_with(" ago") {
            let without_ago = text.strip_suffix(" ago")?;
            let parts: Vec<&str> = without_ago.split_whitespace().collect();
            if parts.len() == 2 {
                let num: i64 = parts[0].parse().ok()?;
                let unit = parts[1];

                let days = match unit {
                    "day" | "days" => num,
                    "week" | "weeks" => num * 7,
                    "month" | "months" => num * 30,
                    "year" | "years" => num * 365,
                    _ => return None,
                };

                // "N days ago" means a point in time, so create a 1-day window around that point
                let target = now - Duration::days(days);
                let start = target - Duration::hours(12);
                let end = target + Duration::hours(12);

                return Some(Self {
                    period: TimePeriod::new(start, end),
                    description: format!("{num} {unit} ago"),
                });
            }
        }

        // "last N days"
        if text.starts_with("last ") && text.ends_with(" days") {
            let num_str = text.strip_prefix("last ")?.strip_suffix(" days")?;
            let days: i64 = num_str.parse().ok()?;
            return Some(Self {
                period: TimePeriod::last_days_at(days, now),
                description: format!("last {days} days"),
            });
        }

        // "last N weeks"
        if text.starts_with("last ") && text.ends_with(" weeks") {
            let num_str = text.strip_prefix("last ")?.strip_suffix(" weeks")?;
            let weeks: i64 = num_str.parse().ok()?;
            return Some(Self {
                period: TimePeriod::last_days_at(weeks * 7, now),
                description: format!("last {weeks} weeks"),
            });
        }

        // Try flexible date parsing first (handles natural dates like "January 15, 2025")
        if let Some(date) = parse_date_flexible(&text) {
            return Some(Self {
                period: TimePeriod::day(date),
                description: text.to_string(),
            });
        }

        // Month name patterns for whole months: "in January 2025", "January 2025", "jan 2025"
        let months = [
            ("january", 1),
            ("jan", 1),
            ("february", 2),
            ("feb", 2),
            ("march", 3),
            ("mar", 3),
            ("april", 4),
            ("apr", 4),
            ("may", 5),
            ("june", 6),
            ("jun", 6),
            ("july", 7),
            ("jul", 7),
            ("august", 8),
            ("aug", 8),
            ("september", 9),
            ("sep", 9),
            ("sept", 9),
            ("october", 10),
            ("oct", 10),
            ("november", 11),
            ("nov", 11),
            ("december", 12),
            ("dec", 12),
        ];

        let text_cleaned = text.strip_prefix("in ").unwrap_or(&text);

        for (month_name, month_num) in months {
            if text_cleaned.starts_with(month_name) {
                // Extract year - must be just "month year" pattern
                let rest = text_cleaned.strip_prefix(month_name)?.trim();
                let year: i32 = rest.parse().ok()?;

                if let Some(period) = TimePeriod::month(year, month_num) {
                    return Some(Self {
                        period,
                        description: format!("{month_name} {year}"),
                    });
                }
            }
        }

        // "since <date>" with flexible date parsing
        if text.starts_with("since ") {
            let date_str = text.strip_prefix("since ")?;
            if let Some(date) = parse_date_flexible(date_str) {
                return Some(Self {
                    period: TimePeriod::new(date.and_hms_opt(0, 0, 0)?.and_utc(), now),
                    description: format!("since {date_str}"),
                });
            }
        }

        // "between X and Y" range syntax
        if text.starts_with("between ") {
            let rest = text.strip_prefix("between ")?;
            // Split on " and " or " to "
            let (start_str, end_str) = if rest.contains(" and ") {
                let parts: Vec<&str> = rest.splitn(2, " and ").collect();
                if parts.len() == 2 {
                    (parts[0].trim(), parts[1].trim())
                } else {
                    return None;
                }
            } else if rest.contains(" to ") {
                let parts: Vec<&str> = rest.splitn(2, " to ").collect();
                if parts.len() == 2 {
                    (parts[0].trim(), parts[1].trim())
                } else {
                    return None;
                }
            } else {
                return None;
            };

            let start_date = parse_date_flexible(start_str)?;
            let end_date = parse_date_flexible(end_str)?;

            // Validate range (start should be before or equal to end)
            if start_date > end_date {
                return None;
            }

            return Some(Self {
                period: TimePeriod::new(
                    start_date.and_hms_opt(0, 0, 0)?.and_utc(),
                    end_date.and_hms_opt(23, 59, 59)?.and_utc(),
                ),
                description: format!("between {start_str} and {end_str}"),
            });
        }

        None
    }

    /// Parse a temporal query with detailed error reporting
    ///
    /// Returns a Result with either the parsed query or a detailed error.
    /// Use this when you need to understand why parsing failed.
    pub fn parse_result(text: &str) -> Result<Self, TemporalParseError> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err(TemporalParseError::EmptyInput);
        }

        // Try the regular parse first
        if let Some(query) = Self::parse(text) {
            return Ok(query);
        }

        let text_lower = trimmed.to_lowercase();

        // Check for patterns that look like date ranges but failed
        if text_lower.starts_with("between ") {
            let rest = text_lower.strip_prefix("between ").unwrap();
            if rest.contains(" and ") {
                let parts: Vec<&str> = rest.splitn(2, " and ").collect();
                if parts.len() == 2 {
                    let start_str = parts[0].trim();
                    let end_str = parts[1].trim();

                    // Check if dates could be parsed
                    let start_parsed = parse_date_flexible(start_str);
                    let end_parsed = parse_date_flexible(end_str);

                    if start_parsed.is_none() {
                        return Err(TemporalParseError::InvalidDate {
                            pattern: text.to_string(),
                            reason: format!("could not parse start date '{start_str}'"),
                        });
                    }
                    if end_parsed.is_none() {
                        return Err(TemporalParseError::InvalidDate {
                            pattern: text.to_string(),
                            reason: format!("could not parse end date '{end_str}'"),
                        });
                    }

                    // If both parsed but we still failed, it's an invalid range
                    if let (Some(start), Some(end)) = (start_parsed, end_parsed) {
                        if start > end {
                            return Err(TemporalParseError::InvalidRange {
                                start: start_str.to_string(),
                                end: end_str.to_string(),
                            });
                        }
                    }
                }
            }
        }

        // Check for patterns that look like "since" but failed
        if text_lower.starts_with("since ") {
            let date_str = text_lower.strip_prefix("since ").unwrap();
            return Err(TemporalParseError::InvalidDate {
                pattern: text.to_string(),
                reason: format!("could not parse date '{date_str}'"),
            });
        }

        // Unrecognized pattern
        Err(TemporalParseError::UnrecognizedPattern {
            input: text.to_string(),
        })
    }
}

/// Filter entities by temporal criteria
mod query;
mod retriever;

pub use query::{
    EntityHistory, EntitySnapshot, HistoryEventType, RelationshipEvent, TemporalEntryResult,
    TemporalRelation, filter_entities_by_time, filter_entries_by_time,
    filter_relationships_by_time,
};
pub use retriever::TemporalRetriever;

#[cfg(test)]
mod tests;
