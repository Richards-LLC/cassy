## Core Concepts

Commander keeps queued supervisor commands and their receipts in the local store across daemon restarts and reexec. Pending commands wait for the next native turn boundary and retain their original device and command reference. A command already claimed for typing is never replayed automatically after a restart. A daemon receipt confirming abandonment before typing shows Not sent. Session end with a potentially stale Queued status stays uncertain and reconciles late submission or outcome evidence.
