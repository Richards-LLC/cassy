### Fixed — Commander

- **Commander tables line up by column** — A table cell that looked like a number used to right-align on its own, so a column mixing "17%" with "8.3 h" zig-zagged, and its header no longer sat over its values. Each column now takes one alignment for its header and every cell. A Markdown alignment row (`:--`, `--:`, `:--:`) sets it; otherwise a column right-aligns only when every value in it is a number. Tables also size to their content instead of stretching one column across the message.
