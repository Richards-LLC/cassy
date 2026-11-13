## Core Concepts

The portable `cas-test-audit` skill produces one read-only inventory of test value, flake risk and the project's own recent flake history. Findings carry source location, pattern, confidence, evidence and a fix. The report distinguishes a named failure/pass on identical source from a workflow flip or issue report, calibrates static candidates with hand review, and routes actionable findings to the approved fix-or-delete batch. It never reruns a suite or changes tests/issues while auditing.
