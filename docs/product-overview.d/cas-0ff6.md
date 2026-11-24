## Core Concepts

A no-code spike's or demo's report is its deliverable. One gate runs before every exemption on close, `task update status=closed`, pre-merge review and release close: the report closes or merges only on an approved independent verdict whose dispatch recorded the proof reference and every committed artifact's id and SHA-256, and only while that record still equals the report. The verifier is handed the recorded proof, and a changed or revoked report retires the stale dispatch so the next attempt requests a new review.
