//! Exactly-once Commander `SendMessage` by `client_ref` (cas-bea6).
//!
//! A browser that loses a send's `MessageQueued` receipt cannot tell whether
//! the message reached the queue. Examples: a half-open socket after a
//! network switch, or a receipt dropped on a reconnect. Resending was unsafe
//! because the daemon enqueued every `SendMessage` it received, so a resend of
//! a delivered message reached the supervisor twice (cas-0978 DIAGNOSIS item 3).
//!
//! The daemon remembers, for a bounded window, the receipt it returned for each
//! `client_ref`. A repeat of the same send returns that first receipt and
//! enqueues nothing. The daemon advertises this as
//! `ProtocolCapability::MessageClientRefDedupe`, so a client may resend
//! unreceipted sends once the capability is present.
//!
//! - The daemon serves one factory session, so the session is implicit.
//! - The key is the sending device and its `client_ref`. Two devices can
//!   never collide, even on a reused ref.
//! - A remembered receipt is returned only when the repeat is the same message
//!   (same target, text, `in_reply_to` and urgency). A ref reused for different
//!   content is a new send.
//! - Only a send that was enqueued is remembered. A send that failed can be
//!   retried and enqueued then.
//! - The table lives in memory. A daemon restart forgets it; that is the
//!   bounded-window trade-off, since a restart also drops the sockets whose
//!   loss a retry recovers from.

use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

/// How long a receipt is remembered: well past a client's receipt timeout
/// (15 s), reconnect ceiling (10 s) and held-send expiry (2 min).
pub(crate) const SEND_DEDUPE_WINDOW: Duration = Duration::from_secs(10 * 60);
/// Receipts kept at most; the oldest go first.
pub(crate) const SEND_DEDUPE_CAPACITY: usize = 1024;

/// One send's identity: which device sent it, under which `client_ref`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct SendKey {
    device_id: String,
    client_ref: String,
}

impl SendKey {
    /// A key only for a send that carries a non-empty `client_ref`; without
    /// one there is nothing a retry could be matched on.
    pub(crate) fn new(device_id: Option<&str>, client_ref: Option<&str>) -> Option<Self> {
        let client_ref = client_ref
            .map(str::trim)
            .filter(|value| !value.is_empty())?;
        Some(Self {
            device_id: device_id.unwrap_or_default().to_string(),
            client_ref: client_ref.to_string(),
        })
    }
}

/// What makes two sends under one key the same message.
pub(crate) fn send_fingerprint(
    target: &str,
    text: &str,
    in_reply_to: Option<i64>,
    urgent: bool,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    target.to_ascii_lowercase().hash(&mut hasher);
    text.hash(&mut hasher);
    in_reply_to.hash(&mut hasher);
    urgent.hash(&mut hasher);
    hasher.finish()
}

/// The receipt a send was answered with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SendReceipt {
    pub(crate) notification_id: i64,
    pub(crate) stamped: bool,
}

#[derive(Debug, Clone)]
struct Remembered {
    fingerprint: u64,
    receipt: SendReceipt,
    at: Instant,
}

/// Bounded memory of recent send receipts.
#[derive(Debug)]
pub(crate) struct SendReceipts {
    entries: HashMap<SendKey, Remembered>,
    order: VecDeque<(SendKey, Instant)>,
    window: Duration,
    capacity: usize,
}

impl Default for SendReceipts {
    fn default() -> Self {
        Self::new(SEND_DEDUPE_WINDOW, SEND_DEDUPE_CAPACITY)
    }
}

impl SendReceipts {
    pub(crate) fn new(window: Duration, capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            window,
            capacity: capacity.max(1),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Drop receipts older than the window.
    fn prune(&mut self, now: Instant) {
        while let Some((key, at)) = self.order.front().cloned() {
            if now.saturating_duration_since(at) < self.window {
                break;
            }
            self.order.pop_front();
            // A key re-remembered later has a newer entry; keep that one.
            if self.entries.get(&key).is_some_and(|entry| entry.at == at) {
                self.entries.remove(&key);
            }
        }
    }

    /// The receipt already given for this same send, if it is still remembered.
    pub(crate) fn lookup(
        &mut self,
        key: &SendKey,
        fingerprint: u64,
        now: Instant,
    ) -> Option<SendReceipt> {
        self.prune(now);
        self.entries
            .get(key)
            .filter(|entry| entry.fingerprint == fingerprint)
            .map(|entry| entry.receipt)
    }

    /// Remember the receipt a send was just answered with.
    pub(crate) fn remember(
        &mut self,
        key: SendKey,
        fingerprint: u64,
        receipt: SendReceipt,
        now: Instant,
    ) {
        self.prune(now);
        self.entries.insert(
            key.clone(),
            Remembered {
                fingerprint,
                receipt,
                at: now,
            },
        );
        self.order.push_back((key, now));
        while self.entries.len() > self.capacity {
            let Some((oldest, at)) = self.order.pop_front() else {
                break;
            };
            if self
                .entries
                .get(&oldest)
                .is_some_and(|entry| entry.at == at)
            {
                self.entries.remove(&oldest);
            }
        }
        // Stale order slots (keys re-remembered since) never outgrow the live entries by much.
        if self.order.len() > self.capacity.saturating_mul(2) {
            let entries = &self.entries;
            self.order
                .retain(|(key, at)| entries.get(key).is_some_and(|entry| entry.at == *at));
        }
    }
}

/// Whether a send was enqueued now or answered from memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SendOutcome {
    Enqueued(SendReceipt),
    Duplicate(SendReceipt),
}

impl SendOutcome {
    pub(crate) fn receipt(self) -> SendReceipt {
        match self {
            Self::Enqueued(receipt) | Self::Duplicate(receipt) => receipt,
        }
    }
}

/// Enqueue a send exactly once per key and fingerprint within the window.
///
/// `enqueue` runs only for a send not already answered. Its receipt is
/// remembered only when it succeeds.
pub(crate) fn dedupe_send(
    receipts: &mut SendReceipts,
    key: Option<SendKey>,
    fingerprint: u64,
    now: Instant,
    enqueue: impl FnOnce() -> anyhow::Result<SendReceipt>,
) -> anyhow::Result<SendOutcome> {
    if let Some(key) = key.as_ref()
        && let Some(receipt) = receipts.lookup(key, fingerprint, now)
    {
        return Ok(SendOutcome::Duplicate(receipt));
    }
    let receipt = enqueue()?;
    if let Some(key) = key {
        receipts.remember(key, fingerprint, receipt, now);
    }
    Ok(SendOutcome::Enqueued(receipt))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(device: &str, client_ref: &str) -> SendKey {
        SendKey::new(Some(device), Some(client_ref)).unwrap()
    }

    #[test]
    fn a_repeat_returns_the_first_receipt_and_enqueues_nothing() {
        let mut receipts = SendReceipts::default();
        let now = Instant::now();
        let fingerprint = send_fingerprint("supervisor", "Do the burn down", None, false);
        let mut calls = 0;
        let mut enqueue = || {
            calls += 1;
            Ok(SendReceipt {
                notification_id: 41,
                stamped: true,
            })
        };
        let first = dedupe_send(
            &mut receipts,
            Some(key("phone", "ref-1")),
            fingerprint,
            now,
            &mut enqueue,
        )
        .unwrap();
        let again = dedupe_send(
            &mut receipts,
            Some(key("phone", "ref-1")),
            fingerprint,
            now + Duration::from_secs(20),
            &mut enqueue,
        )
        .unwrap();
        assert_eq!(
            first,
            SendOutcome::Enqueued(SendReceipt {
                notification_id: 41,
                stamped: true
            })
        );
        assert_eq!(
            again,
            SendOutcome::Duplicate(SendReceipt {
                notification_id: 41,
                stamped: true
            })
        );
        assert_eq!(calls, 1);
    }

    #[test]
    fn no_client_ref_other_device_or_other_content_is_a_new_send() {
        assert_eq!(SendKey::new(Some("phone"), None), None);
        assert_eq!(SendKey::new(Some("phone"), Some("  ")), None);
        let mut receipts = SendReceipts::default();
        let now = Instant::now();
        let text = send_fingerprint("supervisor", "Ship it", None, false);
        let mut next = 0;
        let mut enqueue = || {
            next += 1;
            Ok(SendReceipt {
                notification_id: next,
                stamped: false,
            })
        };
        dedupe_send(
            &mut receipts,
            Some(key("phone", "ref-1")),
            text,
            now,
            &mut enqueue,
        )
        .unwrap();
        // No ref: nothing to match on, every send is enqueued.
        assert!(matches!(
            dedupe_send(&mut receipts, None, text, now, &mut enqueue).unwrap(),
            SendOutcome::Enqueued(_)
        ));
        assert!(matches!(
            dedupe_send(&mut receipts, None, text, now, &mut enqueue).unwrap(),
            SendOutcome::Enqueued(_)
        ));
        // The same ref from another device is another send.
        assert!(matches!(
            dedupe_send(
                &mut receipts,
                Some(key("laptop", "ref-1")),
                text,
                now,
                &mut enqueue
            )
            .unwrap(),
            SendOutcome::Enqueued(_)
        ));
        // The same ref for different content is another send, and it replaces the memory.
        let edited = send_fingerprint("supervisor", "Ship it after the gate", None, false);
        assert!(matches!(
            dedupe_send(
                &mut receipts,
                Some(key("phone", "ref-1")),
                edited,
                now,
                &mut enqueue
            )
            .unwrap(),
            SendOutcome::Enqueued(_)
        ));
        assert_eq!(
            receipts.lookup(&key("phone", "ref-1"), edited, now),
            Some(SendReceipt {
                notification_id: 5,
                stamped: false
            })
        );
        assert_eq!(receipts.lookup(&key("phone", "ref-1"), text, now), None);
        // Target case, reply binding and urgency are part of the message.
        assert_eq!(
            send_fingerprint("Supervisor", "x", None, false),
            send_fingerprint("supervisor", "x", None, false)
        );
        assert_ne!(
            send_fingerprint("supervisor", "x", Some(7), false),
            send_fingerprint("supervisor", "x", None, false)
        );
        assert_ne!(
            send_fingerprint("supervisor", "x", None, true),
            send_fingerprint("supervisor", "x", None, false)
        );
        assert_eq!(next, 5);
    }

    /// End to end on the real prompt queue: the daemon's enqueue behind the
    /// dedupe stores one row for a send and its resend, and both answers name
    /// that row.
    #[test]
    fn a_resend_through_the_real_queue_is_stored_once() {
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        let attribution = crate::ui::factory::protocol::MessageAttribution {
            device_id: Some("phone-7".to_string()),
            credential_id: Some("cred-7".to_string()),
            device_label: Some("Pixel".to_string()),
            operator_label: Some("Daniel".to_string()),
            controller_origin: None,
            request_id: None,
            scopes: vec!["message:send".to_string()],
            operator_verified: true,
        };
        let mut receipts = SendReceipts::default();
        let now = Instant::now();
        let send = |receipts: &mut SendReceipts, client_ref: &str, at: Instant| {
            dedupe_send(
                receipts,
                SendKey::new(attribution.device_id.as_deref(), Some(client_ref)),
                send_fingerprint("supervisor", "Do the burn down", None, false),
                at,
                || {
                    let outcome = super::super::delivery::enqueue_commander_message(
                        &cas_dir,
                        "factory-1",
                        "supervisor",
                        "Do the burn down",
                        None,
                        false,
                        None,
                        &attribution,
                    )?;
                    Ok(SendReceipt {
                        notification_id: outcome.id(),
                        stamped: super::super::delivery::operator_stamp(&attribution).verified,
                    })
                },
            )
            .unwrap()
        };
        let first = send(&mut receipts, "ref-burn-down", now);
        let resent = send(
            &mut receipts,
            "ref-burn-down",
            now + Duration::from_secs(16),
        );
        assert!(matches!(first, SendOutcome::Enqueued(_)));
        assert_eq!(resent, SendOutcome::Duplicate(first.receipt()));
        assert!(first.receipt().stamped);
        let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
        let rows = queue.peek_all(10).unwrap();
        assert_eq!(
            rows.iter()
                .filter(|row| row.prompt.contains("Do the burn down"))
                .count(),
            1
        );
        assert_eq!(
            rows.iter()
                .find(|row| row.prompt.contains("Do the burn down"))
                .unwrap()
                .id,
            first.receipt().notification_id
        );
        // A new send (new client_ref) with the same words is a second message.
        let second = send(
            &mut receipts,
            "ref-burn-down-2",
            now + Duration::from_secs(30),
        );
        assert!(matches!(second, SendOutcome::Enqueued(_)));
        assert_ne!(
            second.receipt().notification_id,
            first.receipt().notification_id
        );
    }

    #[test]
    fn a_failed_send_is_not_remembered() {
        let mut receipts = SendReceipts::default();
        let now = Instant::now();
        let fingerprint = send_fingerprint("supervisor", "Hi", Some(99), false);
        let error = dedupe_send(
            &mut receipts,
            Some(key("phone", "ref-2")),
            fingerprint,
            now,
            || anyhow::bail!("in_reply_to notification 99 does not exist"),
        );
        assert!(error.is_err());
        assert_eq!(receipts.len(), 0);
        let retried = dedupe_send(
            &mut receipts,
            Some(key("phone", "ref-2")),
            fingerprint,
            now,
            || {
                Ok(SendReceipt {
                    notification_id: 7,
                    stamped: true,
                })
            },
        )
        .unwrap();
        assert_eq!(
            retried,
            SendOutcome::Enqueued(SendReceipt {
                notification_id: 7,
                stamped: true
            })
        );
    }

    #[test]
    fn receipts_expire_after_the_window_and_the_oldest_go_past_capacity() {
        let mut receipts = SendReceipts::new(Duration::from_secs(60), 3);
        let start = Instant::now();
        let fingerprint = send_fingerprint("supervisor", "m", None, false);
        let receipt = |id| SendReceipt {
            notification_id: id,
            stamped: false,
        };
        receipts.remember(key("phone", "a"), fingerprint, receipt(1), start);
        assert_eq!(
            receipts.lookup(
                &key("phone", "a"),
                fingerprint,
                start + Duration::from_secs(59)
            ),
            Some(receipt(1))
        );
        assert_eq!(
            receipts.lookup(
                &key("phone", "a"),
                fingerprint,
                start + Duration::from_secs(60)
            ),
            None
        );
        assert_eq!(receipts.len(), 0);
        let later = start + Duration::from_secs(120);
        for (index, client_ref) in ["b", "c", "d", "e"].into_iter().enumerate() {
            receipts.remember(
                key("phone", client_ref),
                fingerprint,
                receipt(index as i64),
                later,
            );
        }
        assert_eq!(receipts.len(), 3);
        assert_eq!(
            receipts.lookup(&key("phone", "b"), fingerprint, later),
            None
        );
        assert_eq!(
            receipts.lookup(&key("phone", "e"), fingerprint, later),
            Some(receipt(3))
        );
        // Re-remembering a key keeps its newest entry through the older slot's eviction.
        receipts.remember(
            key("phone", "c"),
            fingerprint,
            receipt(9),
            later + Duration::from_secs(30),
        );
        assert_eq!(
            receipts.lookup(
                &key("phone", "c"),
                fingerprint,
                later + Duration::from_secs(70)
            ),
            Some(receipt(9))
        );
    }
}
