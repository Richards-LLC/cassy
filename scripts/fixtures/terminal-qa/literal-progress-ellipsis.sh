#!/bin/sh
# terminal-qa fixture: a literal progress sentence ending in an ellipsis.
printf '  Reconfiguring Cassy...\n'
printf 'Installing dependencies...\n'
printf 'Building project...\n'
if [ "${LC_ALL:-}" = C ]; then
  printf 'Fetching updates...\n'
else
  printf 'Installing dependencies…\nFetching updates…\n'
fi
