### Fixed — Config and workspace policy

- **Honor configured write roots** — Moving files into a configured Trash directory now creates children without treating the destination directory as an overwrite. User-level parent roots remain effective in nested projects instead of disappearing from config get and hook policy.
- **Report the effective store and tracker accurately** — Store notices describe CAS_ROOT as the default and account for explicit `--store` selection. SessionStart uses the same issue destination resolver as config get.
- **Keep generated harness files consistent** — Pure agent and rule projections are ignored; mixed configuration remains tracked, regenerated from current source during release preparation. Merge regeneration stages only tracked outputs so ignored native projections cannot stop the merge.
