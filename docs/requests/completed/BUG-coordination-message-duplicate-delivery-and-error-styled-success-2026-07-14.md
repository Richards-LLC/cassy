---
from: Ozer supervisor (pippenz @ /home/pippenz/Petrastella/ozer)
date: 2026-07-14
priority: P3
---

# BUG: Teammate message re-delivered verbatim after being handled; SendMessage auto-route reports success inside an error envelope; asymmetric registration

Three related coordination-bridge issues from one director↔supervisor exchange:

## 1. Duplicate delivery of an already-handled message

The director's "epic cas-ea3e subtasks closed — verify/close/shutdown" message was delivered to my session **twice**, the second time well after I had completed all requested work AND replied (my reply: message id 3083, auto-routed via Cassy). The duplicate was byte-identical, with no redelivery marker. If the bridge re-queues on missing ack, the ack either wasn't recorded or the dedup window is broken; either way the receiver has no way to distinguish "re-sent intentionally" from "queue replay", so every duplicate forces a re-verification pass (tool calls, task reads) to prove it's stale.

## 2. Successful auto-route surfaced as a tool **error**

Calling the builtin `SendMessage(to: "director", …)` returned:

```
<error>✅ AUTO-ROUTED via Cassy coordination (message id 3083). Message delivered to `director`.
DO NOT retry this SendMessage call.</error>
```

A delivered message reported through the error channel (with a ✅ inside) is confusing for agents and any tooling keying on error status — several harness behaviors treat tool errors as failures to retry or surface. If the hook intercepts and routes successfully, it should return a normal success result carrying the "use `coordination action=message` next time" guidance.

## 3. Asymmetric registration

Immediately after receiving a message FROM the director, my `coordination action=message target=director` reply queued with *"target not yet registered, will deliver on registration"*. The director can evidently reach me while being unregistered as a target in the same registry — inbound and outbound identity resolution disagree. (Delivery-on-registration is a fine fallback; the inconsistency is the bug.)

## Environment

- `cas 2.27.0 (dd8bcbd-dirty 2026-07-11)`, factory mode, supervisor `fast-kestrel-14`, team `ozer-keen-hawk-76`, session `07275a32-c0d5-4695-abbb-5c04663df721`
- Director = separate Claude session on the same project, messages arriving as `teammate-message` blocks
