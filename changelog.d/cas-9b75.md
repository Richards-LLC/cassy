### Added — Release safety

- **Browser QA before direct production merges** — A frozen candidate containing deferred rendered work needs a browser PASS on its exact build before Cassy proposes or enqueues it. A receipt from before the production base moved does not count. Staged, API-only, and demo-only deliveries keep their existing workflow.
