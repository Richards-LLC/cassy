### Fixed — Factory

- **Supervisors can record stability proof from the other machine** — `record_test_receipt` accepts `repeat_log_paths` with the four repeat logs of a peer machine's capped run. When all five logs show exactly the changed tests passing, each as its own run, the receipt carries the five-pass stability proof that park requires. Short, failed, reused or mismatched log sets are refused. Previously a changed test that could only run on the other machine could not park without an override.
