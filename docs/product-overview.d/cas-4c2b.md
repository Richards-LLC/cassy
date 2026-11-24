## Core Concepts

When Violet's lookup has no record of a post for this caller (`not_posted` with reason `request_not_found`), Cassy resends it once on its own after a closed connection. Otherwise the error is retryable and says to resend with the same request ID and arguments.
