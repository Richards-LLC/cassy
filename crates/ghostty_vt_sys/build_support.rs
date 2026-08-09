/// Map every supported Cargo target to an explicit Zig baseline.
///
/// Native targets are intentionally mapped too: omitting `-Dtarget` makes Zig
/// optimize for the build host and can leak AVX-512 into distributed binaries.
/// Unknown targets fail closed so a distributable artifact can never silently
/// inherit the build host's instruction set.
pub fn rust_target_to_zig(rust_target: &str) -> Result<&'static str, String> {
    match rust_target {
        "x86_64-unknown-linux-gnu" => Ok("x86_64-linux-gnu"),
        "aarch64-unknown-linux-gnu" => Ok("aarch64-linux-gnu"),
        "x86_64-unknown-linux-musl" => Ok("x86_64-linux-musl"),
        "aarch64-unknown-linux-musl" => Ok("aarch64-linux-musl"),
        "x86_64-apple-darwin" => Ok("x86_64-macos"),
        "aarch64-apple-darwin" => Ok("aarch64-macos"),
        _ => Err(format!(
            "unsupported Cargo target `{rust_target}` for ghostty_vt_sys; refusing to build without an explicit portable Zig target. Supported targets: x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu, x86_64-unknown-linux-musl, aarch64-unknown-linux-musl, x86_64-apple-darwin, aarch64-apple-darwin"
        )),
    }
}
