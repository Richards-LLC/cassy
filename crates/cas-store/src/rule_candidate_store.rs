//! Verifier observations held as rule candidates (cas-f0a1).
//!
//! A verifier lesson from one verdict is not a rule. It is stored here, outside
//! the `rules` table and the search index, so no agent context, search, recall
//! or rule sync can surface it. Each similar observation adds an occurrence
//! linked to its verdict; when a candidate is seen in enough different tasks,
//! the supervisor and operator decide whether it becomes a rule.
use crate::{Result, error::StoreError};
use chrono::Utc;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;

pub const RULE_CANDIDATE_SCHEMA_STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS rule_candidates (
        id TEXT PRIMARY KEY, content TEXT NOT NULL, category TEXT NOT NULL DEFAULT '',
        created_at TEXT NOT NULL)",
    "CREATE TABLE IF NOT EXISTS rule_candidate_occurrences (
        candidate_id TEXT NOT NULL REFERENCES rule_candidates(id), task_id TEXT NOT NULL,
        verdict_id TEXT NOT NULL, observed_at TEXT NOT NULL,
        PRIMARY KEY (candidate_id, task_id, verdict_id))",
];

/// Two observations name the same lesson at or above this word overlap.
pub const RULE_CANDIDATE_MATCH: f64 = 0.5;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleCandidateOccurrence {
    pub task_id: String,
    pub verdict_id: String,
    pub observed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleCandidate {
    pub id: String,
    pub content: String,
    pub category: String,
    pub created_at: String,
    pub occurrences: Vec<RuleCandidateOccurrence>,
}

impl RuleCandidate {
    /// Occurrences count once per task: one task's repeated verdicts are one
    /// piece of evidence.
    pub fn distinct_tasks(&self) -> usize {
        self.occurrences
            .iter()
            .map(|occurrence| occurrence.task_id.as_str())
            .collect::<HashSet<_>>()
            .len()
    }
}

fn words(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| word.len() > 2)
        .map(str::to_lowercase)
        .collect()
}

/// Dice overlap of the two texts' words, from 0.0 to 1.0.
pub fn rule_candidate_similarity(a: &str, b: &str) -> f64 {
    let (a, b) = (words(a), words(b));
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    2.0 * a.intersection(&b).count() as f64 / (a.len() + b.len()) as f64
}

fn connection(root: &Path) -> Result<std::sync::Arc<std::sync::Mutex<rusqlite::Connection>>> {
    let conn = crate::shared_db::shared_connection(&root.join("cas.db"))?;
    {
        let guard = crate::shared_db::lock_connection(&conn)?;
        for sql in RULE_CANDIDATE_SCHEMA_STATEMENTS {
            guard.execute(sql, [])?;
        }
    }
    Ok(conn)
}

fn load(conn: &rusqlite::Connection) -> Result<Vec<RuleCandidate>> {
    let mut candidates = conn
        .prepare("SELECT id, content, category, created_at FROM rule_candidates ORDER BY created_at, id")?
        .query_map([], |row| {
            Ok(RuleCandidate {
                id: row.get(0)?,
                content: row.get(1)?,
                category: row.get(2)?,
                created_at: row.get(3)?,
                occurrences: Vec::new(),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut occurrences = conn.prepare(
        "SELECT task_id, verdict_id, observed_at FROM rule_candidate_occurrences
        WHERE candidate_id=?1 ORDER BY observed_at, task_id, verdict_id",
    )?;
    for candidate in &mut candidates {
        candidate.occurrences = occurrences
            .query_map([&candidate.id], |row| {
                Ok(RuleCandidateOccurrence {
                    task_id: row.get(0)?,
                    verdict_id: row.get(1)?,
                    observed_at: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
    }
    Ok(candidates)
}

pub fn list_rule_candidates(root: &Path) -> Result<Vec<RuleCandidate>> {
    let conn = connection(root)?;
    let guard = crate::shared_db::lock_connection(&conn)?;
    load(&guard)
}

/// Candidates whose lesson matches `content`, most similar first.
pub fn similar_rule_candidates(root: &Path, content: &str) -> Result<Vec<(RuleCandidate, f64)>> {
    let mut similar: Vec<_> = list_rule_candidates(root)?
        .into_iter()
        .map(|candidate| {
            let score = rule_candidate_similarity(&candidate.content, content);
            (candidate, score)
        })
        .filter(|(_, score)| *score >= RULE_CANDIDATE_MATCH)
        .collect();
    similar.sort_by(|a, b| b.1.total_cmp(&a.1));
    Ok(similar)
}

/// Record one verifier observation. It joins `candidate_id` when given, else
/// the most similar existing candidate, else starts a new candidate. A repeat
/// of the same task and verdict is not counted again.
pub fn observe_rule_candidate(
    root: &Path,
    candidate_id: Option<&str>,
    content: &str,
    category: &str,
    task_id: &str,
    verdict_id: &str,
) -> Result<RuleCandidate> {
    let (content, task_id, verdict_id) = (content.trim(), task_id.trim(), verdict_id.trim());
    if content.is_empty() || task_id.is_empty() || verdict_id.is_empty() {
        return Err(StoreError::Parse(
            "a rule candidate observation needs content, a task and a verdict".into(),
        ));
    }
    let conn = connection(root)?;
    let tx = crate::shared_db::begin_immediate_pooled(&conn)?;
    let id = match candidate_id {
        Some(id) => tx
            .query_row("SELECT id FROM rule_candidates WHERE id=?1", [id], |row| row.get(0))
            .optional()?
            .ok_or_else(|| StoreError::NotFound(format!("rule candidate {id}")))?,
        None => match load(&tx)?
            .into_iter()
            .map(|candidate| {
                let score = rule_candidate_similarity(&candidate.content, content);
                (candidate.id, score)
            })
            .filter(|(_, score)| *score >= RULE_CANDIDATE_MATCH)
            .max_by(|a, b| a.1.total_cmp(&b.1))
        {
            Some((id, _)) => id,
            None => {
                let id = format!("rc-{:08x}", rand::random::<u32>());
                tx.execute(
                    "INSERT INTO rule_candidates (id, content, category, created_at)
                    VALUES (?1, ?2, ?3, ?4)",
                    params![id, content, category.trim(), Utc::now().to_rfc3339()],
                )?;
                id
            }
        },
    };
    tx.execute(
        "INSERT OR IGNORE INTO rule_candidate_occurrences
        (candidate_id, task_id, verdict_id, observed_at) VALUES (?1, ?2, ?3, ?4)",
        params![id, task_id, verdict_id, Utc::now().to_rfc3339()],
    )?;
    let candidate = load(&tx)?
        .into_iter()
        .find(|candidate| candidate.id == id)
        .ok_or_else(|| StoreError::NotFound(format!("rule candidate {id}")))?;
    tx.commit()?;
    Ok(candidate)
}
