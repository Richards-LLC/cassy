# Skill mechanics

Provenance: adapted from `mattpocock/skills` (MIT, © 2026 Matt Pocock).

## Invocation

A model-invoked skill has a description: that description is an always-loaded context pointer, so write trigger branches precisely. It remains available to explicit user invocation. Use model invocation when the agent or another skill must discover it autonomously.

A user-invoked skill sets `disable-model-invocation: true`. Its description is human-facing and carries no autonomous trigger. Choose it when a person should decide whether to use the skill, trading context load for cognitive load.

## Splitting and routers

Split a skill only when a distinct leading word needs independent model invocation, or a different invocation boundary protects a sequence from premature completion. A family of user-invoked skills may use one user-invoked router: it helps people find the right skill, but cannot autonomously invoke its peers.
