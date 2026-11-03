//! Read-only policy comparison. A peer snapshot is explicit evidence, never authority.
use std::collections::BTreeMap;
use std::fs;

use chrono::{DateTime, Utc};
use rmcp::model::{CallToolResult, Content, ErrorCode};
use serde::{Deserialize, Serialize};

use crate::mcp::server::CasCore;
use crate::sync::Syncer;
use cas_types::Rule;

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct CloudPolicy {
    auto_sync: bool,
    pull_on_start: bool,
    interval_secs: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct RuleSnapshot {
    machine: String,
    project: String,
    captured_at: DateTime<Utc>,
    cloud: Option<CloudPolicy>,
    rules: Vec<Rule>,
}

impl CasCore {
    pub(crate) async fn cas_rule_drift(
        &self,
        peer_snapshot: Option<String>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let invalid = |message| Self::error(ErrorCode::INVALID_PARAMS, message);
        let failure = |error: String| Self::error(ErrorCode::INTERNAL_ERROR, error);
        let config = self.load_config();
        let project_root = self.cas_root.parent().unwrap_or(&self.cas_root);
        let target = project_root.join(&config.sync.target);
        let syncer = Syncer::new(target.clone(), config.sync.min_helpful);
        let rules = self
            .open_rule_store()?
            .list()
            .map_err(|e| failure(format!("Cannot read rule source: {e}")))?;
        let local = RuleSnapshot {
            machine: config
                .cloud
                .as_ref()
                .and_then(|c| c.machine_label.clone())
                .unwrap_or_else(|| {
                    hostname::get()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|_| "unknown".into())
                }),
            project: crate::cloud::resolve_canonical_id(&self.cas_root).ok_or_else(|| {
                invalid("Cannot identify this project's rule snapshot".to_string())
            })?,
            captured_at: Utc::now(),
            cloud: config.cloud.as_ref().map(|c| CloudPolicy {
                auto_sync: c.auto_sync,
                pull_on_start: c.pull_on_start,
                interval_secs: c.interval_secs,
            }),
            rules,
        };
        let mut findings = Vec::new();
        if local.cloud.is_none() {
            findings.push("cloud: no [cloud] sync section configured locally".to_string());
        } else if local
            .cloud
            .as_ref()
            .is_some_and(|c| !c.auto_sync || !c.pull_on_start)
        {
            findings.push("cloud: automatic sync or startup pull is disabled locally".to_string());
        }
        let mut syncable = local.rules.clone();
        crate::store::foreign_project_guard::ForeignProjectGuard::for_project_root(project_root)
            .retain_syncable_rules(&mut syncable);
        let expected: BTreeMap<_, _> = syncable
            .iter()
            .filter(|r| syncer.should_sync(r))
            .map(|r| (r.id.clone(), Syncer::render_rule(r)))
            .collect();
        let mut generated = BTreeMap::new();
        match fs::read_dir(&target) {
            Ok(entries) => {
                for entry in entries {
                    let path = entry.map_err(|e| failure(e.to_string()))?.path();
                    if path.extension().is_some_and(|e| e == "md") {
                        let id = path
                            .file_stem()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned();
                        generated.insert(
                            id,
                            fs::read_to_string(&path).map_err(|e| {
                                failure(format!(
                                    "Cannot read generated rule {}: {e}",
                                    path.display()
                                ))
                            })?,
                        );
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(failure(format!(
                    "Cannot read generated rule directory: {e}"
                )));
            }
        }
        for (id, content) in &expected {
            match generated.get(id) {
                None => findings.push(format!("{id}: generated file is missing")),
                Some(actual) if actual != content => {
                    findings.push(format!("{id}: generated content differs from source"))
                }
                _ => {}
            }
        }
        for id in generated.keys().filter(|id| !expected.contains_key(*id)) {
            findings.push(format!("{id}: generated file has no active source rule"));
        }
        let mut peer_label =
            "Peer comparison: NOT EXERCISED (no peer_snapshot supplied)".to_string();
        if let Some(payload) = peer_snapshot {
            let peer: RuleSnapshot = serde_json::from_str(&payload)
                .map_err(|e| invalid(format!("Invalid peer_snapshot: {e}")))?;
            if peer.project != local.project {
                return Err(invalid(format!(
                    "Peer project {} differs from local project {}",
                    peer.project, local.project
                )));
            }
            peer_label = format!(
                "Peer: {} (snapshot captured {})",
                peer.machine, peer.captured_at
            );
            if peer.cloud != local.cloud {
                findings.push(format!(
                    "cloud: sync settings differ from peer {}",
                    peer.machine
                ));
            }
            let by_id = |rules: Vec<Rule>| -> Result<BTreeMap<String, Rule>, rmcp::ErrorData> {
                let mut map = BTreeMap::new();
                for rule in rules {
                    let id = rule.id.clone();
                    if map.insert(id.clone(), rule).is_some() {
                        return Err(invalid(format!("Duplicate rule {id} in snapshot")));
                    }
                }
                Ok(map)
            };
            let local_rules = by_id(local.rules.clone())?;
            let peer_rules = by_id(peer.rules)?;
            for (id, rule) in &local_rules {
                match peer_rules.get(id) {
                    None => findings.push(format!("{id}: absent on peer {}", peer.machine)),
                    Some(other) => {
                        if rule.status != other.status {
                            findings.push(format!(
                                "{id}: status {} locally, {} on peer {}",
                                rule.status, other.status, peer.machine
                            ));
                        }
                        if rule.content != other.content
                            || rule.paths != other.paths
                            || rule.scope != other.scope
                            || rule.stakeholder != other.stakeholder
                            || rule.review_after != other.review_after
                        {
                            findings.push(format!(
                                "{id}: content or scope differs from peer {}",
                                peer.machine
                            ));
                        }
                    }
                }
            }
            for id in peer_rules
                .keys()
                .filter(|id| !local_rules.contains_key(*id))
            {
                findings.push(format!("{id}: present only on peer {}", peer.machine));
            }
        }
        let mut output = format!(
            "Rule drift: {} finding(s); report only\nSource: {}\nGenerated: {}\n{}\n",
            findings.len(),
            self.cas_root.display(),
            target.display(),
            peer_label
        );
        for finding in &findings {
            output.push_str(&format!("- {finding}\n"));
        }
        output.push_str("Export structured_content.snapshot for a peer's rule action=drift peer_snapshot. Snapshots describe their capture time; no live peer check or policy change is implied.");
        Ok(CallToolResult {
            content: vec![Content::text(output)],
            structured_content: Some(serde_json::json!({"snapshot": local, "findings": findings})),
            is_error: Some(false),
            meta: None,
        })
    }
}
