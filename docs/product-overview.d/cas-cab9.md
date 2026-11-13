## Core Concepts

Commander history reads use an independent read-only SQLite connection, including
earlier sessions, open questions and delivery receipts. A daemon writer holding the
shared connection no longer prevents those reads. The conversation shows measured
phase status or a known waiting question; absent or unrecognized phases add no
placeholder or guessed activity line.
