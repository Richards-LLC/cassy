### Fixed — Release notes lint

- **The release-notes lint refuses drafts the Violet hub refuses as unreadable** — Was: `release-train-announce.py --lint` passed a draft that named a plain file such as DESIGN.md or showed a UTC time, then Violet refused the post as format_unreadable. Now: the lint applies the hub's unattached-file and UTC-time rules to every body (names inside links and fenced code still pass), so a draft that lints clean posts.
