//! Hash-based store IDs and their clock boundary.

use chrono::Utc;

#[cfg(test)]
thread_local! {
    static FIXED_TIMESTAMP: std::cell::Cell<Option<i64>> = const { std::cell::Cell::new(None) };
}

pub(crate) fn timestamp_nanos() -> Option<i64> {
    #[cfg(test)]
    if let Some(timestamp) = FIXED_TIMESTAMP.get() {
        return Some(timestamp);
    }
    Utc::now().timestamp_nanos_opt()
}

#[cfg(test)]
pub(crate) fn with_fixed_timestamp<T>(timestamp: i64, body: impl FnOnce() -> T) -> T {
    struct Restore(Option<i64>);
    impl Drop for Restore {
        fn drop(&mut self) {
            FIXED_TIMESTAMP.set(self.0);
        }
    }
    let _restore = Restore(FIXED_TIMESTAMP.replace(Some(timestamp)));
    body()
}
