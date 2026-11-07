### Fixed — Developer workflow

- **Wording changes no longer decide delivery** — Removed phrase registries, vocabulary lists and copied-message checks from tests and delivery admission. Error checks keep behavior or machine identifiers, and park rejects new human-message assertions. Worker and verifier guidance now follows the same rule. Product output checks are unchanged: customer reports still refuse internal process words, tool names and placeholders, and the release announcement lint still refuses internal words in user threads.
