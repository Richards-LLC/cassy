# SHIPPED — macOS `statvfs` type mismatch in tmpfs guardrail

Shipped in commit `e35e530a6ae8cad16f3c308412a7223cb89d536f`.

## Bug

The Unix `fs_usage` implementation multiplied raw statvfs(3) fields without
normalizing their widths. On macOS arm64, `f_frsize` is `u64` while `f_blocks`
and `f_bfree` are `u32`, so the saturating arithmetic failed to compile with
an operand type mismatch. Linux uses compatible field widths and did not
expose the issue.

## Resolution

`f_frsize`, `f_blocks`, and `f_bfree` are now converted to `u64` before the
saturating subtraction and multiplication.

## Verification

- `cargo build -p cas`
- `cargo test -p cas tmpfs_guardrail -- --nocapture` (16 passed)
