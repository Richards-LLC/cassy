### Fixed — Factory

- **Shell guard allows a counting loop** — A worker shell command that counts
  with a local variable, such as
  `missing=0; for p in a b; do [ -f "$p" ] || missing=$((missing+1)); done`,
  is no longer refused as an unresolved arithmetic operand. Cassy proves the
  counter numeric first: it must start as a literal-integer assignment
  statement of its own (not an argument, a prefix assignment or an inherited
  value), every later assignment must be such a statement of an integer or
  `$((...))`, and no command in the script may
  assign it another way. Every reassignment, including one inside arithmetic,
  must draw only on other proven counters. Quoted, escaped or heredoc text never
  counts as arithmetic, and a script with comments, `$'...'` quoting or
  backslash-newline continuations proves no counter. Any other arithmetic over
  variables is still refused.
