### Changed — Bug intake

- **Findings and QA follow-ups find before they file** — Was: a finding whose
  wrong output an open task already recorded was refused until the filer
  retried with `confirm_warning=true`. A matching open GitHub issue was never
  checked, so the finding or QA follow-up became a new task. Now: both paths
  use the report_cas_bug matcher against the project's issue tracker and open
  tasks. A match gets the evidence and a +1, the existing issue or task is
  returned, and no task is created.
