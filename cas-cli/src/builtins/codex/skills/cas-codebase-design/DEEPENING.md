# Deepening

Classify dependencies before combining shallow modules: in-process dependencies
can be tested through the new interface directly; local-substitutable ones use
their real local stand-in; owned remote dependencies justify an injected port
and in-memory/transport adapters; true externals use an injected mock adapter.

Keep internal seams private. Replace old shallow-module tests with tests at the
deepened interface once that interface covers the behavior; do not layer tests
that assert internals or must change with every refactor.
