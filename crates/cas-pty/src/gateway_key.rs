//! Home-gateway client key handling. The key lives only in its private file
//! and the child process environment; configs, argv and logs carry the path.

/// Launch metadata naming the key file. `Pty::spawn` consumes it and never
/// passes it to the child.
pub const GATEWAY_KEY_FILE_ENV: &str = "CAS_FACTORY_GATEWAY_KEY_FILE";
/// Variable a gateway Codex worker reads through its provider `env_key`.
pub const CODEX_GATEWAY_KEY_ENV: &str = "CAS_GATEWAY_API_KEY";

/// Read the client key. Refuses a file readable by group or others, and a
/// key that could not travel as one header value. Errors never echo the key.
pub fn read_gateway_key(path: &std::path::Path) -> Result<String, String> {
    let shown = path.display();
    let metadata = std::fs::metadata(path).map_err(|error| format!("key file {shown}: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("key file {shown} is not a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = metadata.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            return Err(format!("key file {shown} has mode {mode:o}; it must be 0600"));
        }
    }
    let raw = std::fs::read_to_string(path).map_err(|error| format!("key file {shown}: {error}"))?;
    let key = raw.trim();
    if key.is_empty() || key.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(format!("key file {shown} must hold one key on one line"));
    }
    Ok(key.to_string())
}
