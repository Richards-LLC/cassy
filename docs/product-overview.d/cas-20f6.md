## Core Concepts

A burst of directed messages to the supervisor is delivered as one turn. While the batch waits for its last delivery claim, it keeps every message it holds. A later queue refresh cannot send one of those messages on its own.
