### Fixed — Factory

- **Drained inboxes stop delivery wakes** — The delivery watchdog retires a notification after it is read or acknowledged, including worker and supervisor inboxes. A still-unread message keeps its normal wake, now naming the notification it asks the recipient to read.
