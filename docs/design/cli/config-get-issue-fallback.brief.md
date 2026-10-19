# Config get: blank Cassy issue component

| Field | Decision |
| --- | --- |
| First two lines | Keep the config scalar on stdout and explain an explicit blank component on stderr, so the operator sees why bug filing selects the stored project tracker. |
| Scannable | The ordinary get remains one scalar; only the raw-blank opt-out adds one short ASCII note. |
| Readable | The note names the legacy issues.repo fallback, or says that the project override is unset and the component default applies. |
| Machine output | JSON stays one document with its existing fields; a raw-blank Cassy component adds raw_blank, report_repo and report_source, using the same stored-only selection as report_cas_bug. |
| Omitted | No inferred origin is described as a bug-filing fallback, and ordinary configured or absent components receive no extra note. |

## Critique

Terminal captures and scores are pending the executed proof. Source or fixture
checks will not be described as real-build terminal QA.
