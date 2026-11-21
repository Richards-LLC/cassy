## Core Concepts

- **Epic CI evidence.** Was: an intentionally skipped post-merge job blocked
  epic close even when the PR had tested the identical tree. Now: required
  check identities and an exact merge-source tree receipt can establish that
  proof. Missing metadata, untested changes and unsuccessful checks still
  block close; project-specific CI names are supported.
