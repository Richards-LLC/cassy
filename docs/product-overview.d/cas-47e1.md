## Core Concepts

- **Filename discovery** — Workers can use standalone `rg --files` and
  name-only `find` commands under credential-store parent directories, including
  output redirected to a file. Pipelines and substitutions retain ordinary
  credential inspection; explicit credential-store operands remain protected.
