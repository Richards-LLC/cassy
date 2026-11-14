### Fixed — Factory

- **CI reminder workflow names shared by several workflows** — A `ci_completed` reminder whose workflow name matches several workflows, such as the suggested `"CI"`, binds to the workflow that ran for the commit. When the commit's runs do not single one out, the refusal lists each candidate's file path and ID.
