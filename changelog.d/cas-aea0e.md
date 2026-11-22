### Fixed — Factory

- **H-09 no longer refuses dependency logging to an artifacts variable** — Was:
  a worker running `pnpm audit` and a background `pnpm install` that wrote
  their logs to `"$A/..."`, with `A` set earlier in the same command, was
  refused as operator credential access when the command's directory could
  not be resolved. Now: a variable the command binds is judged at its value,
  so those writes are allowed; a credential store read through such a
  variable, relative or absolute, is refused, including one the guard used
  to miss.
