//! A scoped, per-thread view of the process environment (cas-7cc95).
//!
//! Retrieval evaluation ranks memories through the production hook path,
//! which reads identity and host settings (`CAS_*`, `HOME`, `XDG_CONFIG_HOME`)
//! from the process environment. It used to neutralise them by mutating the
//! process environment around each call, which races any other thread and
//! cannot be shared with a test-suite guard. Instead, a caller builds an
//! immutable [`HookEnvironment`] and installs it for the current thread with
//! [`HookEnvironment::scope`]; the readers on that path call [`var_os`] /
//! [`var`] / [`lookup`], which consult the innermost installed environment
//! before the process environment. Nothing here ever writes the process
//! environment, and a thread with no installed environment reads the process
//! environment exactly as before.
//!
//! The scope is per thread: work the scoped call hands to another thread does
//! not see it. The ranking path is synchronous on its calling thread; see the
//! retrieval-eval tests that pin this.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::marker::PhantomData;
use std::sync::Arc;

/// An immutable environment view: explicit values, plus keys and prefixes
/// that read as unset. Keys not mentioned fall through to the process.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookEnvironment {
    values: BTreeMap<OsString, OsString>,
    unset_keys: Vec<OsString>,
    unset_prefixes: Vec<String>,
}

impl HookEnvironment {
    pub fn new() -> Self {
        Self::default()
    }

    /// Read `key` as `value`.
    pub fn with_var(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        self.values
            .insert(key.as_ref().to_os_string(), value.as_ref().to_os_string());
        self
    }

    /// Read `key` as unset unless [`Self::with_var`] gives it a value.
    pub fn without_var(mut self, key: impl AsRef<OsStr>) -> Self {
        self.unset_keys.push(key.as_ref().to_os_string());
        self
    }

    /// Read every key starting with `prefix` as unset unless given a value.
    pub fn without_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.unset_prefixes.push(prefix.into());
        self
    }

    /// This environment's decision for `key`: `Some(Some(value))` set,
    /// `Some(None)` unset, `None` not decided (read the process).
    pub fn decide(&self, key: &OsStr) -> Option<Option<OsString>> {
        if let Some(value) = self.values.get(key) {
            return Some(Some(value.clone()));
        }
        let unset = self.unset_keys.iter().any(|unset| unset == key)
            || key.to_str().is_some_and(|key| {
                self.unset_prefixes
                    .iter()
                    .any(|prefix| key.starts_with(prefix.as_str()))
            });
        unset.then_some(None)
    }

    /// Install this environment for the current thread until the guard
    /// drops. Scopes stack: an inner scope shadows an outer one and the outer
    /// one is back in force when the inner guard drops, including during a
    /// panic unwind. The guard is neither `Send` nor `Sync`, so it cannot
    /// leave the thread it was installed on.
    pub fn scope(self: &Arc<Self>) -> HookEnvironmentScope {
        ACTIVE.with(|active| active.borrow_mut().push(Arc::clone(self)));
        let depth = ACTIVE.with(|active| active.borrow().len());
        HookEnvironmentScope {
            depth,
            _not_send: PhantomData,
        }
    }
}

thread_local! {
    static ACTIVE: RefCell<Vec<Arc<HookEnvironment>>> = const { RefCell::new(Vec::new()) };
}

/// Restores the previous environment view when dropped (see
/// [`HookEnvironment::scope`]).
#[must_use = "the environment is only installed while the guard lives"]
pub struct HookEnvironmentScope {
    depth: usize,
    _not_send: PhantomData<*const ()>,
}

impl Drop for HookEnvironmentScope {
    fn drop(&mut self) {
        ACTIVE.with(|active| {
            let mut active = active.borrow_mut();
            // Guards drop in reverse order of creation on one thread, so
            // this guard's entry is the top one; truncating to below it also
            // removes anything an inner guard leaked with mem::forget.
            active.truncate(self.depth.saturating_sub(1));
        });
    }
}

/// Whether an environment view is installed on this thread.
pub fn is_scoped() -> bool {
    ACTIVE.with(|active| !active.borrow().is_empty())
}

static UNSCOPED_READS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static SCOPED_READS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// How many overlay-aware reads ran under an installed environment: proof
/// that a scoped path actually consults it.
#[doc(hidden)]
pub fn scoped_reads() -> usize {
    SCOPED_READS.load(std::sync::atomic::Ordering::SeqCst)
}

/// How many overlay-aware reads ran on a thread with no installed
/// environment. A scoped call whose work hopped to another thread would
/// raise this, so tests pin that a scoped ranking leaves it unchanged.
#[doc(hidden)]
pub fn unscoped_reads() -> usize {
    UNSCOPED_READS.load(std::sync::atomic::Ordering::SeqCst)
}

/// The innermost installed environment's decision for `key`, if any.
pub fn lookup(key: impl AsRef<OsStr>) -> Option<Option<OsString>> {
    ACTIVE.with(|active| {
        let active = active.borrow();
        match active.last() {
            Some(environment) => {
                SCOPED_READS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                environment.decide(key.as_ref())
            }
            None => {
                UNSCOPED_READS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                None
            }
        }
    })
}

/// [`std::env::var_os`] seen through the installed environment.
pub fn var_os(key: impl AsRef<OsStr>) -> Option<OsString> {
    let key = key.as_ref();
    match lookup(key) {
        Some(decided) => decided,
        None => std::env::var_os(key),
    }
}

/// [`std::env::var`] seen through the installed environment.
pub fn var(key: impl AsRef<OsStr>) -> Result<String, std::env::VarError> {
    match var_os(key) {
        Some(value) => value
            .into_string()
            .map_err(std::env::VarError::NotUnicode),
        None => Err(std::env::VarError::NotPresent),
    }
}

/// The home directory seen through the installed environment: its `HOME`
/// when it decides one (unset reads as no home), otherwise `fallback()`.
pub fn home_dir(fallback: impl FnOnce() -> Option<std::path::PathBuf>) -> Option<std::path::PathBuf> {
    match lookup("HOME") {
        Some(decided) => decided.map(std::path::PathBuf::from),
        None => fallback(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unscoped_reads_are_the_process_environment() {
        assert!(!is_scoped());
        assert_eq!(var_os("PATH"), std::env::var_os("PATH"));
        assert_eq!(lookup("PATH"), None);
    }

    #[test]
    fn a_scope_sets_unsets_and_falls_through_then_restores() {
        let environment = Arc::new(
            HookEnvironment::new()
                .without_prefix("CAS_")
                .without_var("HOME")
                .with_var("HOME", "/neutral/home")
                .with_var("CAS_ROOT", "/neutral/cas"),
        );
        {
            let _scope = environment.scope();
            assert_eq!(var("HOME").as_deref(), Ok("/neutral/home"));
            assert_eq!(var("CAS_ROOT").as_deref(), Ok("/neutral/cas"));
            assert_eq!(var_os("CAS_AGENT_ROLE"), None, "a scrubbed prefix reads as unset");
            assert_eq!(var_os("PATH"), std::env::var_os("PATH"), "unmentioned keys fall through");
            assert_eq!(home_dir(|| None), Some("/neutral/home".into()));
        }
        assert!(!is_scoped());
        assert_eq!(var_os("HOME"), std::env::var_os("HOME"));
    }

    #[test]
    fn scopes_stack_and_unwind_restores_the_outer_scope() {
        let outer = Arc::new(HookEnvironment::new().with_var("CAS_AGENT_ROLE", "outer"));
        let inner = Arc::new(HookEnvironment::new().with_var("CAS_AGENT_ROLE", "inner"));
        let _outer = outer.scope();
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _inner = inner.scope();
            assert_eq!(var("CAS_AGENT_ROLE").as_deref(), Ok("inner"));
            panic!("a ranking failure unwinds through the scope");
        }));
        assert!(unwound.is_err());
        assert_eq!(var("CAS_AGENT_ROLE").as_deref(), Ok("outer"), "the panic restored the outer scope");
    }

    #[test]
    fn concurrent_threads_see_only_their_own_scope() {
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let process_home = std::env::var_os("HOME");
        let handles: Vec<_> = ["a", "b"]
            .into_iter()
            .map(|name| {
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let environment = Arc::new(HookEnvironment::new().with_var("CAS_AGENT_NAME", name));
                    let _scope = environment.scope();
                    barrier.wait();
                    (0..1000)
                        .map(|_| var("CAS_AGENT_NAME"))
                        .all(|seen| seen.as_deref() == Ok(name))
                })
            })
            .collect();
        for handle in handles {
            assert!(handle.join().unwrap(), "each thread saw only its own scope");
        }
        assert_eq!(std::env::var_os("HOME"), process_home, "the process environment was never touched");
    }
}
