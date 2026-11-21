## Core Concepts

Task read offsets are UTF-8 byte cursors for show, get and notes. Task list
rejects a supplied offset instead of silently returning its first page again;
list callers select results with filters and a limit.
