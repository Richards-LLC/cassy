### Fixed — Violet replies

- **A `not_posted` reply names its cause and next step** — When Violet confirmed a post did not land but gave no reason, Cassy returned `not_posted` with nothing to act on, and supervisors had to guess that a fresh reply intent would work. The error now carries `reason` (`provider_unreported` when Violet gives none) and `next_action`: `new_reply_intent` for a managed reply, `new_request_id` for a plain message, or `resend_same_request_id` when Violet never received the request (GH #1968).
