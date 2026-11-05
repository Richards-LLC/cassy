### Fixed

- A worker can close a delivery whose pre-merge approval was recorded on another machine. Confirmed publication shares an authenticated peer receipt that the receiving daemon validates and keeps in its existing verification records, independently of mailbox prompt retention. Changed delivery identities, forged senders and replay into a new proof cycle remain refused.
