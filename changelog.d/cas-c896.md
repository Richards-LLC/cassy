### Fixed — Factory merges

- **Keep approval while a candidate rebuild continues** — an unchanged delivery's approval now survives a candidate rebuild that exceeds the response deadline. Retrying the merge reuses the existing approval after preparation finishes. Disjoint train changes retain the original dispatch and record the carried approval on the task; changed deliveries and conflicts retain their existing review checks.
