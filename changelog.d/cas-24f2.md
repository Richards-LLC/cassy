### Fixed — Worker shell guards

Worker guards share the shell scanner and distinguish a shell's command string
from arguments passed to a literal script. SQL arguments after a wrapper script
are data. Arithmetic can use local numeric bindings established by a literal
assignment prefix, with conservative invalidation for later writes, branches,
loops, input, array assignments and variable attributes.

Unsupported stdin shell input, deferred code, dynamic commands and eval, and
coprocesses now receive named grammar refusals. Argument projections retain
literal provenance and distinguish case patterns and test brackets from command
words. Build and signal guards consume the common bounded execution walk;
input-derived xargs filenames require explicit operands.
Role checks use the same executing prefixes and wrapper traversal, retaining
the distinction between shell syntax and literal arguments.

Compound boundaries, list operands and redirect targets receive grammar
refusals when incomplete, and compound nesting uses the shared depth limit.
Literal filename-list pipelines through proven text filters retain metadata
classification; filename-opening consumers and protected inputs keep their
credential checks. Ripgrep text mode works for policy-documentation searches.

Credential checks use the shared shell input boundaries and admitted nesting
limit. Bash numeric tests consume data while actual arithmetic keeps its
checks. Literal entry-directory canonicalization preserves artifact-root
containment, and pattern/rule-file inputs retain their credential checks.

Git scope checks consume shared statement provenance, so unrelated quoted data
does not alter a later syntax-check invocation. Shell payload boundaries stay
consistent across Git, credential and role checks.
