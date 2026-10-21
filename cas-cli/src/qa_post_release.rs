//! Read-only post-release observations, persisted independently of publication.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub epic: String,
    pub sha: String,
    pub started_at: DateTime<Utc>,
    pub window_mins: u32,
    pub interval_mins: u32,
    pub runs: Vec<Run>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub at: DateTime<Utc>,
    pub output: Option<CheckOutput>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckOutput {
    pub ok: bool,
    pub alarms: Vec<Alarm>,
    pub checked_at: DateTime<Utc>,
    pub readback: Option<Readback>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alarm {
    pub id: String,
    pub summary: String,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Readback {
    pub journey: String,
    pub subject: String,
    pub invariants: Vec<String>,
    pub ok: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict { Pending, Clean, Failed }

impl Receipt {
    pub fn verdict(&self, _now: DateTime<Utc>) -> Verdict { Verdict::Pending }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn qa_post_release_verdict_sequences() {
        let start = DateTime::parse_from_rfc3339("2026-10-21T00:00:00Z").unwrap().with_timezone(&Utc);
        let good = json!({"ok":true,"alarms":[],"checked_at":start,
            "readback":{"journey":"signup","subject":"opaque-1","invariants":["workspace"],"ok":true}});
        let no_readback = json!({"ok":true,"alarms":[],"checked_at":start});
        let mut alarm = good.clone();
        alarm["alarms"] = json!([{"id":"missing","summary":"Missing workspace","count":1}]);
        let mut bad_readback = good.clone();
        bad_readback["readback"]["ok"] = json!(false);
        let mut not_ok = good.clone();
        not_ok["ok"] = json!(false);
        for (name, minutes, outputs, expected) in [
            ("open", 10, vec![Some(good.clone())], Verdict::Pending),
            ("clean", 30, vec![Some(good.clone()),Some(no_readback.clone()),Some(good.clone())], Verdict::Clean),
            ("alarm", 1, vec![Some(alarm)], Verdict::Failed),
            ("error", 1, vec![None], Verdict::Failed),
            ("readback failed", 1, vec![Some(bad_readback)], Verdict::Failed),
            ("not ok", 1, vec![Some(not_ok)], Verdict::Failed),
            ("missing readback", 30, vec![Some(no_readback.clone());3], Verdict::Failed),
            ("missing run", 31, vec![Some(good)], Verdict::Failed),
        ] {
            let receipt = Receipt { epic:"cas-epic".into(), sha:"a".repeat(40), started_at:start,
                window_mins:30, interval_mins:15,
                runs:outputs.into_iter().enumerate().map(|(i,output)| Run { at:start + chrono::Duration::minutes(i as i64*15),
                    error:output.is_none().then(||"timeout".into()), output:output.map(|v|serde_json::from_value(v).unwrap()) }).collect() };
            assert_eq!(receipt.verdict(start + chrono::Duration::minutes(minutes)),expected,"{name}");
        }
    }
}
