### Fixed — Factory

- **H-09 names the supported form for dependency logging, and refuses store reads through a variable** — Was:
  a worker that changed into a directory Cassy could not resolve, then ran
  `pnpm audit` or a background `pnpm install` logging to an artifacts
  variable, got a bare operator-credential refusal with no way forward;
  and a credential store read through a variable set in the same command
  was allowed. Now: that refusal says to `cd` to the literal project
  directory first, which is allowed; the operand spelling is still judged
  exactly as before, and a value a variable is set to can only add a
  refusal, so a store read through it is refused.
