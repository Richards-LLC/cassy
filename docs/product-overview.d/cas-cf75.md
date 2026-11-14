## Core Concepts

Search returns the last committed results while another writer owns the task search index. The next search reconciles canonical task rows once the writer is available; uncommitted changes remain invisible.
