## Core Concepts

Factory deliveries automatically arm supervisor-owned CI and published-merge completion events when parked or sent for merge. The events survive session restarts, deduplicate repeated requests and recognize new CI attempts on an unchanged commit. The progress watchdog reports an unwatched delivery during its next cycle instead of waiting for its stall timer.
