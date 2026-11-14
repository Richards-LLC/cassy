### Preserve shell input boundaries and local numeric provenance

Worker guards share the shell scanner and distinguish a shell's command string
from arguments passed to a literal script. SQL arguments after a wrapper script
are data. Arithmetic can use local numeric bindings established by a literal
assignment prefix, with conservative invalidation for later writes, branches,
loops, input, array assignments and variable attributes.
