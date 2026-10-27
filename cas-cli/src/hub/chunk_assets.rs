//! Shared by build.rs and the hub: Vite root chunks only, never paths.

/// `chunk-[name]-[hash].js|css`, with Vite's default eight-character
/// base64url hash. ASCII validation also rejects traversal, escapes and NUL.
pub(crate) fn content_type(name: &str) -> Option<&'static str> {
    let (stem, mime) = if let Some(stem) = name.strip_suffix(".js") {
        (stem, "text/javascript; charset=utf-8")
    } else if let Some(stem) = name.strip_suffix(".css") {
        (stem, "text/css; charset=utf-8")
    } else {
        return None;
    };
    let stem = stem.strip_prefix("chunk-")?;
    if !stem
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return None;
    }
    let separator = stem.len().checked_sub(9)?;
    (separator > 0 && stem.as_bytes()[separator] == b'-').then_some(mime)
}
