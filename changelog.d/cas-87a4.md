### Fixed — Factory

- **Shell guard allows a counting loop** — A worker shell command that counts
  with a local variable, such as
  `missing=0; for p in a b; do [ -f "$p" ] || missing=$((missing+1)); done`,
  is no longer refused as an unresolved arithmetic operand. Cassy proves the
  counter numeric first: it must start as a literal integer, every later
  assignment must be an integer or `$((...))`, and no command in the script may
  assign it another way. Any other arithmetic over variables is still refused.
