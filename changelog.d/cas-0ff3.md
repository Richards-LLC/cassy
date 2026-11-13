## Preserve peer task status during session startup

Session startup no longer reopens tasks from another machine merely because they have no local lease. Orphan recovery requires a previous local holder that still matches the task assignment, and retains the task if its state changes during recovery.
