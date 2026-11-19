### Added — Factory

- **A repair verdict that names failing tests must also name their class** — Was: a supervisor could send a delivery back naming only two failing tests, and a third test of the same class failed on the next park. Now: `task action=request_changes` takes `tests_failed` and `class_audit`; naming failing tests without a valid class filter is refused with the remedy, the class is stored on the task, and park refuses until a passing capped receipt at the tip runs the whole class.
