### Fixed — Releases

- **Train CI reruns and early failures** — Re-running only failed Rust shards now retains successful shards from earlier attempts of the same run. A failed train run without a receipt records an explicit unknown red result, wakes its owner and holds merges; release cuts report the missing receipt until a verified green clears the hold.
