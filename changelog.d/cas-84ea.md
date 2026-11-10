### Fixed — Worker commit guard

- **Inspect the commit's actual directory** — Compound worker commands using relative directory changes or prior literal path variables now resolve the correct branch. Unresolved directories and Git read failures report uncertainty instead of a false detached-HEAD diagnosis.
