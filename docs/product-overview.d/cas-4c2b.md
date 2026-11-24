## Core Concepts

When Violet reports that it never received a post (`not_posted` with reason `request_not_found`), the reply intent is still unused. Cassy resends once on its own. If that resend also fails, the error is retryable and says to resend the same request ID with unchanged arguments.
