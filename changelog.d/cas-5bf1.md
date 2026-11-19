### Added — Factory

- **Supervisor briefs get a testing-policy warning** — Was: a spawn brief, a `request_changes` reason or a task's acceptance criteria could ask for a test per item or for assertions on message text, which contradicts the project's testing rules, and nothing said so. Now: each of the three returns a warning code (`N-106` for message or output text assertions, `N-107` for a test per item) beside its normal receipt. It never refuses, and a sentence that forbids the ask does not trigger it. The cas-supervisor brief-fields reference also carries a required Test plan line.
