### Fixed — Batch QA close

- **A reopened delivery closes on the receipt that already covers it** — Was: after a post-merge reopen cleared the active branch anchor, closing answered "waiting for batch receipt" even though `qa_status` reported the covering receipt as PASS. Now: close falls back to the delivery's latest retired anchor, the same one `qa_status` uses, so the two agree.
