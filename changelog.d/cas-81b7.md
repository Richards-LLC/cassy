### Fixed — Developer checks

- **Disposable Rust fixture edits** — Was: test-shape lint refused a source read used only to append to a disposable runtime fixture. Now: an immediate same-path formatted rewrite is accepted when the read value has no other use; source assertions, comparisons and aliases remain refused.
