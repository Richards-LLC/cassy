//! Same-thread notice ownership for a native update. Only notices for retired
//! settings are deferred; parsing, policy and every other diagnostic stay live.
use crate::config::Config;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub(crate) struct RetiredConfigNotice {
    pub path: PathBuf,
    pub keys: BTreeSet<String>,
}

#[derive(Default)]
struct Pending {
    notices: BTreeMap<PathBuf, RetiredConfigNotice>,
    represented: BTreeSet<PathBuf>,
}

thread_local! {
    static PENDING: RefCell<Option<Pending>> = const { RefCell::new(None) };
}

/// Nested sweeps borrow the outer command's scope. Only its owner can drain
/// it, so a helper returning early cannot discard the caller's notices.
pub(crate) struct RetiredNoticeScope {
    owner: bool,
    replay: fn(Vec<RetiredConfigNotice>, bool),
    verbose: bool,
}

impl RetiredNoticeScope {
    pub(crate) fn begin(replay: fn(Vec<RetiredConfigNotice>, bool), verbose: bool) -> Self {
        let owner = PENDING.with_borrow_mut(|pending| {
            if pending.is_some() {
                false
            } else {
                *pending = Some(Pending::default());
                true
            }
        });
        Self {
            owner,
            replay,
            verbose,
        }
    }

    /// Return a bounded discovery worker's notices to the command that owns
    /// them. Its later project phases can still mark those notices represented.
    pub(crate) fn adopt(
        notices: Vec<RetiredConfigNotice>,
        replay: fn(Vec<RetiredConfigNotice>, bool),
    ) {
        let remaining = PENDING.with_borrow_mut(|pending| {
            let Some(pending) = pending else {
                return notices;
            };
            for notice in notices {
                if !pending.represented.contains(&notice.path) {
                    pending
                        .notices
                        .entry(notice.path.clone())
                        .or_insert_with(|| RetiredConfigNotice {
                            path: notice.path,
                            keys: BTreeSet::new(),
                        })
                        .keys
                        .extend(notice.keys);
                }
            }
            Vec::new()
        });
        if !remaining.is_empty() {
            replay(remaining, false);
        }
    }

    pub(crate) fn take(&mut self) -> Vec<RetiredConfigNotice> {
        if !std::mem::take(&mut self.owner) {
            return Vec::new();
        }
        PENDING.with_borrow_mut(|pending| {
            pending
                .take()
                .unwrap_or_default()
                .notices
                .into_values()
                .collect()
        })
    }
}

impl Drop for RetiredNoticeScope {
    fn drop(&mut self) {
        let notices = self.take();
        if !notices.is_empty() {
            (self.replay)(notices, self.verbose);
        }
    }
}

fn config_path(root: &Path) -> PathBuf {
    root.canonicalize()
        .unwrap_or_else(|_| root.to_owned())
        .join("config.toml")
}

pub(crate) fn represented_retired_config(root: &Path) {
    PENDING.with_borrow_mut(|pending| {
        if let Some(pending) = pending {
            let path = config_path(root);
            pending.notices.remove(&path);
            pending.represented.insert(path);
        }
    });
}

pub(super) fn defer_retired_config(root: &Path, config: &Config) -> bool {
    PENDING.with_borrow_mut(|pending| {
        let Some(pending) = pending else {
            return false;
        };
        let path = config_path(root);
        if pending.represented.contains(&path) {
            return true;
        }
        let mut keys: BTreeSet<_> = config
            .factory
            .as_ref()
            .map(|factory| {
                factory
                    .retired_keys()
                    .into_iter()
                    .map(|key| format!("factory.{key}"))
                    .collect()
            })
            .unwrap_or_default();
        if let Some(notifications) = &config.notifications {
            keys.extend(
                notifications
                    .retired_keys()
                    .into_iter()
                    .map(|key| format!("notifications.{key}")),
            );
        }
        if config.retired_jev.is_some() {
            keys.insert("jev".into());
        }
        if !keys.is_empty() {
            pending
                .notices
                .entry(path.clone())
                .or_insert_with(|| RetiredConfigNotice {
                    path,
                    keys: BTreeSet::new(),
                })
                .keys
                .extend(keys);
        }
        true
    })
}
