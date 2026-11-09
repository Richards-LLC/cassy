### Fixed — Build capacity notices

- **Quiet priority queues and group disk waits** — Workers queued behind higher-priority builds no longer wake the supervisor. Disk-pressure waits share one notice per factory session after 20 minutes, listing waiting workers and reclaimable target caches. Later notices require a changed waiting set or reclaimable-byte total, with durable deduplication across daemon restarts.
