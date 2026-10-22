//! Shared provider budgets (cas-2d91, design findings 17 and 22). The
//! registry header declares `resources` and journeys declare `consumes`.
//! Rate and quota resources are metered per request in one ledger per
//! (env, resource) in `.cas/cas.db`, which every process on the host shares.
//! Balance resources are checked once at preflight through the adapter's
//! `balance` verb, and a shortfall is covered with `grant` in `grant_envs` or
//! refused before the freeze. A 429 or 402 from the provider is
//! [`BlockedExternal`].
use super::blocked::BlockedExternal;
use super::{CallError, ErrorKind, ReleaseConfig, Request, Verb, call, printable};
use crate::qa_registry::{Journey, Limit, Resource};
use cas_store::{ResourceGrant, ResourceTake};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

const MINUTE_MS: i64 = 60_000;
const DAY_MS: i64 = 86_400_000;
/// Longest pause between attempts while another request holds a concurrency slot.
const HELD_POLL: Duration = Duration::from_millis(250);

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// One planned journey: it uses `consumes × runs × cells` of each resource.
#[derive(Debug, Clone, Copy)]
pub struct PlannedRow<'a> {
    pub journey: &'a Journey,
    pub runs: u32,
    pub cells: u32,
}

/// What preflight measured and granted, for the evidence pack.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ResourcePlan {
    pub env: String,
    /// Σ consumes × runs × cells per resource.
    pub need: BTreeMap<String, u64>,
    /// Each balance resource after any grant.
    pub balance: BTreeMap<String, u64>,
    /// The shortfall granted per balance resource.
    pub granted: BTreeMap<String, u64>,
}

/// Why the gate must not freeze. Every refusal precedes the freeze.
#[derive(Debug, thiserror::Error)]
pub enum ResourceRefusal {
    #[error(
        "RESOURCE UNDECLARED: {journey} consumes {resource}, which registry resources do not declare"
    )]
    Undeclared { journey: String, resource: String },
    #[error("RESOURCE INVALID: {resource}: {message}")]
    Invalid { resource: String, message: String },
    /// A balance resource is short and was not (or not fully) granted.
    #[error("RESOURCE SHORTFALL: {resource} needs {need}, balance {balance} on {env}; {why}")]
    Balance {
        resource: String,
        need: u64,
        balance: u64,
        env: String,
        why: String,
    },
    /// A quota cannot cover the plan within today's remaining allowance.
    #[error(
        "RESOURCE SHORTFALL: {resource} needs {need}, {left} of {per_day} left in 24h on {env}"
    )]
    Quota {
        resource: String,
        need: u64,
        left: u64,
        per_day: u32,
        env: String,
    },
    /// One run uses more than a rate resource allows per minute, so it never fits.
    #[error(
        "RESOURCE SHORTFALL: {journey} needs {need} {resource} per run, above per_min {per_min}"
    )]
    Rate {
        journey: String,
        resource: String,
        need: u32,
        per_min: u32,
    },
    /// `balance` or `grant` failed; [`ResourceRefusal::blocked`] classifies it.
    #[error("RESOURCE UNKNOWN: {0}")]
    Call(#[from] CallError),
    #[error("RESOURCE LEDGER: {0}")]
    Ledger(String),
}

impl ResourceRefusal {
    /// A provider-caused `balance`/`grant` failure (429, 402, 5xx).
    pub fn blocked(&self) -> Option<BlockedExternal> {
        match self {
            Self::Call(CallError::Adapter(_, error)) => BlockedExternal::from_adapter(error),
            _ => None,
        }
    }
}

fn limit(
    resources: &BTreeMap<String, Resource>,
    journey: &str,
    name: &str,
) -> Result<Limit, ResourceRefusal> {
    let resource = resources
        .get(name)
        .ok_or_else(|| ResourceRefusal::Undeclared {
            journey: journey.into(),
            resource: name.into(),
        })?;
    resource
        .limit()
        .map_err(|message| ResourceRefusal::Invalid {
            resource: name.into(),
            message,
        })
}

/// Gate preflight: sums the plan's need, refuses what the local ledger can
/// never meet, then reads `balance` and grants the shortfall in
/// `grant_envs`. Local refusals make no adapter call; a plan without a
/// balance resource makes none either.
pub fn preflight(
    project_root: &Path,
    cas_root: &Path,
    config: &ReleaseConfig,
    env: &str,
    sha: &str,
    resources: &BTreeMap<String, Resource>,
    rows: &[PlannedRow<'_>],
) -> Result<ResourcePlan, ResourceRefusal> {
    let mut plan = ResourcePlan {
        env: env.into(),
        ..ResourcePlan::default()
    };
    let mut limits = BTreeMap::new();
    for row in rows {
        let multiplier = u64::from(row.runs) * u64::from(row.cells);
        for (name, &amount) in &row.journey.consumes {
            let limit = limit(resources, &row.journey.id, name)?;
            if let Limit::Rate { per_min, .. } = limit
                && amount > per_min
            {
                return Err(ResourceRefusal::Rate {
                    journey: row.journey.id.clone(),
                    resource: name.clone(),
                    need: amount,
                    per_min,
                });
            }
            *plan.need.entry(name.clone()).or_default() += u64::from(amount) * multiplier;
            limits.insert(name.as_str(), limit);
        }
    }
    let mut balances = Vec::new();
    for (&name, &limit) in &limits {
        let need = plan.need[name];
        match limit {
            Limit::Quota { per_day } => {
                let used = cas_store::resource_used(cas_root, env, name, now_ms() - DAY_MS)
                    .map_err(|e| ResourceRefusal::Ledger(e.to_string()))?;
                let left = u64::from(per_day).saturating_sub(used);
                if need > left {
                    return Err(ResourceRefusal::Quota {
                        resource: name.into(),
                        need,
                        left,
                        per_day,
                        env: printable(env),
                    });
                }
            }
            Limit::Balance if need > 0 => balances.push(name),
            Limit::Rate { .. } | Limit::Balance => {}
        }
    }
    if balances.is_empty() {
        return Ok(plan);
    }
    let request = |args: Value| Request {
        env,
        sha,
        args,
        ..Request::default()
    };
    let data = call(
        project_root,
        config,
        Verb::Balance,
        request(serde_json::json!({ "resources": balances })),
    )?;
    plan.balance = amounts(Verb::Balance, &data, &balances)?;
    let short: BTreeMap<&str, u64> = balances
        .iter()
        .filter_map(|&name| {
            plan.need[name]
                .checked_sub(plan.balance[name])
                .filter(|s| *s > 0)
                .map(|s| (name, s))
        })
        .collect();
    if short.is_empty() {
        return Ok(plan);
    }
    let shortfall = |name: &str, plan: &ResourcePlan, why: String| ResourceRefusal::Balance {
        resource: name.into(),
        need: plan.need[name],
        balance: plan.balance[name],
        env: printable(env),
        why,
    };
    if !config.grant_envs.iter().any(|allowed| allowed == env) {
        let (name, _) = short.first_key_value().expect("non-empty shortfall");
        let why = format!("{} is not in qa.release.grant_envs", printable(env));
        return Err(shortfall(name, &plan, why));
    }
    let names: Vec<&str> = short.keys().copied().collect();
    let data = call(
        project_root,
        config,
        Verb::Grant,
        request(serde_json::json!({ "amounts": short })),
    )?;
    let granted = amounts(Verb::Grant, &data, &names)?;
    for (&name, &shortfall_amount) in &short {
        plan.balance.insert(name.into(), granted[name]);
        plan.granted.insert(name.into(), shortfall_amount);
    }
    match names
        .iter()
        .find(|&&name| plan.balance[name] < plan.need[name])
    {
        Some(name) => Err(shortfall(name, &plan, "grant did not cover it".into())),
        None => Ok(plan),
    }
}

/// `{resource: amount}` with a non-negative integer for every requested name.
fn amounts(verb: Verb, data: &Value, names: &[&str]) -> Result<BTreeMap<String, u64>, CallError> {
    names
        .iter()
        .map(|&name| {
            data.get(name)
                .and_then(Value::as_u64)
                .map(|amount| (name.to_owned(), amount))
                .ok_or_else(|| {
                    CallError::Protocol(
                        verb.as_str(),
                        format!(
                            "{} data has no whole amount for {}",
                            verb.as_str(),
                            printable(name)
                        ),
                    )
                })
        })
        .collect()
}

/// One non-blocking take from the (env, resource) ledger at `now_ms`.
/// Balance resources are not metered per request and are always granted.
pub(crate) fn try_take(
    cas_root: &Path,
    env: &str,
    name: &str,
    limit: Limit,
    amount: u32,
    now_ms: i64,
    hold: Duration,
) -> cas_store::Result<Option<ResourceGrant>> {
    let (window_ms, limit, concurrency) = match limit {
        Limit::Rate {
            per_min,
            concurrency,
        } => (MINUTE_MS, per_min, concurrency),
        Limit::Quota { per_day } => (DAY_MS, per_day, None),
        Limit::Balance => return Ok(None),
    };
    let hold_ms = i64::try_from(hold.as_millis()).unwrap_or(i64::MAX / 2);
    cas_store::take_resource(
        cas_root,
        &ResourceTake {
            env,
            resource: name,
            amount,
            now_ms,
            window_ms,
            limit,
            concurrency,
            hold_until_ms: now_ms.saturating_add(hold_ms),
        },
    )
    .map(Some)
}

/// Ledger rows taken for one request. Dropping releases their concurrency
/// holds; their use keeps counting in its window.
pub(crate) struct Permits<'a> {
    cas_root: &'a Path,
    env: &'a str,
    /// Ledger row id and resource name.
    ids: Vec<(i64, String)>,
}

impl Permits<'_> {
    /// The request never ran: remove its use as well.
    fn refund(mut self) {
        self.release(true);
    }

    /// A failed release keeps the slot held until its hold expires.
    fn release(&mut self, refund: bool) {
        for (id, resource) in self.ids.drain(..) {
            if let Err(error) = cas_store::release_resource(self.cas_root, id, refund) {
                tracing::warn!(
                    resource = %resource,
                    env = %self.env,
                    refund,
                    %error,
                    "release-gate resource slot not released; it stays held until its hold expires"
                );
            }
        }
    }
}

impl Drop for Permits<'_> {
    fn drop(&mut self) {
        self.release(false);
    }
}

/// Where and until when a row's requests are metered.
#[derive(Debug, Clone, Copy)]
pub struct Meter<'a> {
    pub cas_root: &'a Path,
    pub env: &'a str,
    pub resources: &'a BTreeMap<String, Resource>,
    /// The row's wall clock; no wait or retry runs past it.
    pub deadline: Instant,
}

/// Why a metered call returned no data.
#[derive(Debug, thiserror::Error)]
pub enum MeteredError {
    /// The provider throttled (429, after the one retry the clock allowed),
    /// ran out (402) or failed.
    #[error("{0}")]
    Blocked(BlockedExternal),
    /// The local budget has no room before the wall clock.
    #[error("NOT_EXERCISED{{timed_out}}: no {resource} budget before the wall clock")]
    Budget { resource: String },
    #[error(transparent)]
    Refused(#[from] ResourceRefusal),
    #[error(transparent)]
    Call(CallError),
}

impl From<CallError> for MeteredError {
    fn from(error: CallError) -> Self {
        match &error {
            CallError::Adapter(_, adapter) => {
                BlockedExternal::from_adapter(adapter).map_or(Self::Call(error), Self::Blocked)
            }
            _ => Self::Call(error),
        }
    }
}

fn acquire<'a>(
    meter: &Meter<'a>,
    config: &ReleaseConfig,
    journey: &str,
    consumes: &BTreeMap<String, u32>,
) -> Result<Permits<'a>, MeteredError> {
    let hold = Duration::from_secs(u64::from(config.timeout_secs.max(1)) + 5);
    let mut permits = Permits {
        cas_root: meter.cas_root,
        env: meter.env,
        ids: Vec::new(),
    };
    // BTreeMap order: every process takes resources in the same order.
    for (name, &amount) in consumes {
        let limit = limit(meter.resources, journey, name)?;
        loop {
            let now = now_ms();
            let grant = try_take(meter.cas_root, meter.env, name, limit, amount, now, hold)
                .map_err(|e| ResourceRefusal::Ledger(e.to_string()))?;
            let after = |at: i64| Duration::from_millis((at - now).max(1) as u64);
            let (earliest, latest) = match grant {
                None => break,
                Some(ResourceGrant::Granted(id)) => {
                    permits.ids.push((id, name.clone()));
                    break;
                }
                Some(ResourceGrant::RetryAt { earliest, latest }) => {
                    (after(earliest), after(latest))
                }
                // No wait can fit it: the same refusal preflight gives.
                Some(ResourceGrant::Never) => {
                    permits.refund();
                    return Err(match limit {
                        Limit::Quota { per_day } => ResourceRefusal::Quota {
                            resource: name.clone(),
                            need: u64::from(amount),
                            left: u64::from(per_day),
                            per_day,
                            env: printable(meter.env),
                        },
                        Limit::Rate { per_min, .. } => ResourceRefusal::Rate {
                            journey: journey.into(),
                            resource: name.clone(),
                            need: amount,
                            per_min,
                        },
                        // try_take never meters a balance.
                        Limit::Balance => ResourceRefusal::Invalid {
                            resource: name.clone(),
                            message: "a balance is not metered per request".into(),
                        },
                    }
                    .into());
                }
            };
            let left = meter.deadline.saturating_duration_since(Instant::now());
            if earliest > left {
                permits.refund();
                return Err(MeteredError::Budget {
                    resource: name.clone(),
                });
            }
            // The window frees at `earliest`; a held slot may free any time
            // before `latest`, so poll it.
            std::thread::sleep(latest.min(earliest.max(HELD_POLL)).min(left));
        }
    }
    Ok(permits)
}

/// Calls `verb` for `journey` once its `consumes` fit the shared budget. A
/// 429 is retried once after `retry_after_s` when the wall clock allows;
/// otherwise it, a 402 and any other provider failure are
/// [`MeteredError::Blocked`].
pub fn call_metered(
    project_root: &Path,
    config: &ReleaseConfig,
    meter: &Meter<'_>,
    journey: &Journey,
    verb: Verb,
    request: Request<'_>,
) -> Result<Value, MeteredError> {
    let mut retried = false;
    loop {
        let permits = acquire(meter, config, &journey.id, &journey.consumes)?;
        let answer = call(project_root, config, verb, request.clone());
        drop(permits);
        let retry = match &answer {
            Err(CallError::Adapter(_, error))
                if error.kind == ErrorKind::RateLimited && !retried =>
            {
                error.retry_after_s.map(Duration::from_secs)
            }
            _ => None,
        };
        match retry {
            Some(wait) if Instant::now() + wait < meter.deadline => {
                retried = true;
                std::thread::sleep(wait);
            }
            _ => return answer.map_err(MeteredError::from),
        }
    }
}
