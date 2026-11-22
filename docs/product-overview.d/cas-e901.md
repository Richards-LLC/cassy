## Core Concepts

Literal supervisor messages resolve the worker's live session owner from one registry snapshot. Unavailable or competing owners return structured reasons and supervisor status, session and heartbeat details. Same-name restart ordering and legitimate sessionless fallback remain supported, while metadata cannot borrow an explicitly different factory's supervisor.
