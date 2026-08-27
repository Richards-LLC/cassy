# 2026-08-27 — Capability-aware model routing — #cas-internal

> EMBARGO: do not post before 2026-08-31 (operator confirmation required).
> Draft complete; append the POSTED receipt table after publication.

## User thread

**Top-level (Live on production · User):**

🧭 Cassy now knows which AI accounts your machine actually has and routes work
accordingly — asking for "a light worker" or "a taste worker" just picks the
right model, and a missing account gets you a clear answer instead of a
mysterious failure.

**Reply (Was → Now):**

Was: model choice lived in a doc humans had to remember, enforcement was
scattered, and a machine missing one of the AI accounts failed confusingly
when work was sent to it.

Now: routing policy is one checked, versioned rulebook built into Cassy. Work
can be requested by lane — light, standard, taste, heavy — and each lane maps
to a vetted model at a vetted strength (light: Haiku 4.5; standard: GPT-5.6
Luna; taste: Claude Opus 5; heavy: GPT-5.6 Sol). Health checks report which
backends this machine can actually use — available, unavailable (with the
exact command to enable it), or unknown — and if a preferred backend is
missing, the substitute is announced, never silent. Asking for an exact model
still works and is never swapped behind your back; suspended models are
refused everywhere with the reason. The docs' routing table is generated from
the same rulebook the code enforces, so guidance can no longer drift from
reality.

## Dev thread

**Top-level (Live on production · Dev):**

⚙️ Typed lane registry (embedded TOML) + `cas_factory::routing` enforcement on
every spawn path, tri-state capability snapshots, and a `lane=` request mode
with warned fallback (PR #599).

**Reply (Was → Now):**

Was: Terra/Luna policy hand-coded in the MCP spawn handler only (direct CLI
launches bypassed it); docs pinned by hand-written marker tests; harness
lists hard-coded in three places; no lane abstraction; no account-capability
awareness.

Now: `crates/cas-factory/policy/lane-registry.toml` (schema-versioned,
semantically validated — unknown refs/cycles fatal) defines recipes and lanes;
`cas_factory::routing::validate_explicit` runs post-cascade on every path —
MCP spawn, direct CLI, daemon respawn, doctor, queue consumption, launch —
with rejections naming the violated rule and active alternatives.
`CapabilitySnapshot` is tri-state (Available/Unavailable/Unknown) with TTLs,
keyed by harness+provider+endpoint+model+account profile; doctor/preflight
enumerate all four harnesses from the registry. `lane=` resolves through
registry candidates against the snapshot: fallback warns and names the
substitution, Unknown availability fails closed, taste ships with fallback
disabled, lane+explicit mixing is rejected, explicit specs are immutable.
Route tables and copyable recipes in the supervisor docs are rendered from
the registry with registry-derived golden tests. OpenCode is cataloged with a
lane-unassigned, capability-gated `qwencloud_qwen` recipe; registry policy
validates before the OpenCode receipt-gated route preflight.

## POSTED

(to be filled at publication — parent/reply permalinks + timestamps for both threads)
