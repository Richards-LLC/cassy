### Fixed — Factory

- **Class-audit park hint names the right test target** — When park refuses for a missing class-audit receipt, the suggested capped command now uses the Cargo target that registers the class, such as `--test integration_factory`, instead of always `--lib`. When no target can be resolved, it asks for the registered harness.
