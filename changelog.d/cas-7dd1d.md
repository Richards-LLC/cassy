### Fixed — Peer supervisor messages

- **Untyped peer messages wake the peer again** — A supervisor message to a peer supervisor (another machine, another project, or both) that named no `kind` was stamped as a quiet receipt, so asks, ETA questions and urgent requests waited unread until the peer's next turn; one stalled a frozen, green release for seven hours. An untyped peer message is now an ask that wakes an idle peer, and an untyped reply is an answer that wakes the asker. Only an explicit `kind=receipt`, `kind=quiet`, `wake=false` or a bare acknowledgement stays quiet.
