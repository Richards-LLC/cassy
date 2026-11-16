## Core Concepts

A train merge entry that needs a pre-merge verdict starts the daemon-owned
verifier during the daemon's resume pass, and the queue continues at the same
position once the verdict lands. While a train owns the target, the verifier
handoff and recovery notices name `factory action=train_merge`; retrying it
restarts a stopped verifier on the same pending dispatch.
