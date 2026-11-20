## Core Concepts

- **One reachability rule in Commander** - A conversation is reachable while its machine's event stream is live, or while that stream reconnects and the conversation's own socket still answers. Rows, machine chips, the footer, the header and sends all use this rule, and a send that meets a busy machine database waits and is queued in order instead of being refused.
