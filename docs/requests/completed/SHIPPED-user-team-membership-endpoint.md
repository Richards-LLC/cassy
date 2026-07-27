---
from: Petra Stella Cloud team
date: 2026-05-15
re: FEATURE-user-team-membership-endpoint.md (your cas-ab88)
---

# SHIPPED — GET /api/me is live on epic branch

Endpoint is live on `epic/expose-authenticated-user-identity-team-membership-cas-5370` (commit `c41895a`).

## Final shape (no drift from RESPONSE-user-team-membership-endpoint.md)

```
GET /api/me
Authorization: Bearer <psc_k1_...>

200 OK
{
  "user_id": "<uuid>",
  "email": "daniel@petrastella.io",
  "teams": [
    {
      "id": "2a57bec9-5dfa-4a8f-b711-31f9aeb8d6cb",
      "slug": "petra-stella",
      "name": "Petra Stella",
      "role": "owner"
    }
  ],
  "default_team_id": "2a57bec9-5dfa-4a8f-b711-31f9aeb8d6cb"
}

401 { "error": "..." }  — on invalid / missing token
200 { "user_id", "email", "teams": [], "default_team_id": null }  — zero memberships
```

## Implementation notes

- `default_team_id` computed server-side: `owner > admin > member`, tiebreak oldest `joined_at`.
- `role` included as promised.
- No `plan`, `project_count`, `member_count`, `joined_at` in this endpoint (lean identity shape).
- Auth via existing `psc_k1_*` Bearer token — no changes needed on CLI auth side.
- 35 test files / 180 tests green.

CLI work on cas-ab88 is unblocked.
