## Core Concepts

The daemon-owned pre-merge verifier on the Codex recipe pre-approves only
Cassy's own MCP server, keeping its read-only sandbox. A merge retry on a
pending dispatch whose last verifier attempt failed reports that one
replacement is scheduled on the same dispatch, and the factory daemon logs a
refused verifier launch.
