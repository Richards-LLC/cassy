# Feature file template

One filled example of a `docs/qa/features/<feature>.md` file. Copy the five
H2 headings exactly; the static check (`scripts/check-feature-map.mjs`)
refuses a file missing any of them. Keep each file short enough to read
before a QA pass.

Conventions the check relies on:

- **Touches**: one bullet per source glob, in backticks, relative to the
  project root. Every glob must match at least one file.
- **Driving it**: routes and selectors in backticks (`/settings/profile`,
  `[data-testid="save-profile"]`, `#email`). The check warns when one no
  longer appears in the Touches files. Commands containing spaces are not
  checked.
- **How to get to it**: every entry point on its own bullet. Each is a
  separate matrix row in a QA pass.

## Example: `docs/qa/features/profile-settings.md`

```markdown
# Profile settings

A signed-in user edits their display name, email and avatar.

## Sub-features

- Edit display name
- Change email (sends a confirmation link; the old email stays active until confirmed)
- Upload or remove avatar (PNG or JPEG, up to 2 MB)

## How to get to it

- Avatar menu, top right → "Settings" → "Profile" tab
- Direct link from the "Complete your profile" banner on the dashboard
- URL: `/settings/profile`

## Driving it

1. Sign in with the QA account from `docs/qa/verify.md` (Drive section).
2. Open `/settings/profile`.
3. Fill `#display-name`, then click `[data-testid="save-profile"]`.
4. Expect the toast "Profile saved" and the new name in the avatar menu.

## Gotchas

- The email change is not applied until the link in the confirmation email is
  opened; the local mail catcher is in `docs/qa/verify.md` (Observe).
- Avatar upload is refused silently over 2 MB in Safari; test with a small file.

## Touches

- `src/app/settings/profile/**`
- `src/components/ProfileForm.tsx`
- `src/server/api/profile.ts`
```

## Index line in `docs/qa/features/README.md`

```markdown
- [Profile settings](profile-settings.md): name, email and avatar editing
```
