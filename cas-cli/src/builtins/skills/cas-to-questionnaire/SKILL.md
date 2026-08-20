---
name: cas-to-questionnaire
description: Turn a decision the user cannot answer alone into a discovery questionnaire for a knowledgeable third party.
disable-model-invocation: true
managed_by: cas
---

# Discovery Questionnaire

Imported and adapted from mattpocock/skills `to-questionnaire`, MIT © 2026 Matt Pocock.

Turn an unanswered decision into a document a knowledgeable recipient can complete asynchronously or in a meeting. **Grill the send, not the subject:** establish the recipient’s role, expertise, and relationship, then establish the decisions or facts the user needs back. Do not ask the user for facts the recipient is meant to supply.

Draft a discovery questionnaire in the user-approved output location. State its purpose, sender, recipient, and how answers will be used; give only enough context to orient the recipient; order one-idea questions by importance; provide an answer stub and a short “why this matters” only where useful; finish with a catch-all. Preserve task or decision context through `mcp__cas__task`, `mcp__cas__spec`, or `mcp__cas__memory`, not a parallel tracker or context-file convention.

Before delivery, verify every stated decision gap has a question and report the created path. Do not invent a deadline, recipient knowledge, or a real-world process step that the user has not supplied.
