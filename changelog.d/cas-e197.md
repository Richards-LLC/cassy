### Fixed — Factory

- **Workspace guard accepts read-only counting loops** — A shell command that counts with `n=$((n+1))` from `n=0`, or reads `${!v}` over a literal list of names, is now judged by where it writes. A value inherited from the environment, read from input or produced by a command substitution still keeps arithmetic and indirection refused.
