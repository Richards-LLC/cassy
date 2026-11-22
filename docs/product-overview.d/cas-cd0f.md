## Core Concepts

A coordination send that outlives its response deadline is never left without an identifier. It keeps running under a `send-` continuation id returned with `mutation_outcome=IN_PROGRESS`; `message_status id=<continuation id>` answers IN_PROGRESS, COMMITTED with the notification id, NOT_COMMITTED, or UNKNOWN when no record survives, and a `send_outcome` notice delivers the final outcome to the sender.
