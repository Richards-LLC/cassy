### Fixed — Pre-merge review

- **Report a complete simplicity finding class together** — When a verifier finds unnecessary code, it checks the rest of that file for the same class and reports all instances in one verdict, avoiding a separate re-park for each instance.
