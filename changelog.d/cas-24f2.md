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
