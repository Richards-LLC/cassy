## Core Concepts

A coordination send that outlives its response deadline is never left without an identifier. It keeps running under a `send-` continuation id returned with `mutation_outcome=IN_PROGRESS`; `message_status id=<continuation id>` answers IN_PROGRESS, COMMITTED with the notification id, NOT_COMMITTED only when the send returned a refusal before any write, or UNKNOWN when no write was confirmed, and a `send_outcome` notice delivers the final outcome to the sender.
