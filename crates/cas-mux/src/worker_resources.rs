//! Factory worker MCP configuration and explicit resource denials.

use std::io::Write;
use std::path::{Path, PathBuf};

use cas_types::factory_worker_policy::FactoryWorkerPolicy;

/// Read the same flattened factory policy used by the CLI configuration.
/// Invalid configuration is a launch error, never an empty denial policy.
pub fn load_worker_policy(cas_root: Option<&Path>) -> std::io::Result<FactoryWorkerPolicy> {
    let Some(root) = cas_root else {
        return Ok(Default::default());
    };
    let contents = match std::fs::read_to_string(root.join("config.toml")) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Default::default()),
        Err(error) => return Err(error),
    };
    let document: toml::Value = toml::from_str(&contents)
        .map_err(|_| std::io::Error::other("invalid factory resource configuration"))?;
    document
        .get("factory")
        .cloned()
        .unwrap_or(toml::Value::Table(Default::default()))
        .try_into()
        .map_err(|_| std::io::Error::other("invalid supervisor-only MCP/environment lists"))
}

/// Remove supervisor-only definitions before resolving credential references.
pub(crate) fn filter_proxy_document(document: &mut toml::Value, policy: &FactoryWorkerPolicy) {
    for field in ["servers", "worker_access"] {
        if let Some(servers) = document.get_mut(field).and_then(toml::Value::as_table_mut) {
            servers.retain(|name, _| !policy.denies_server(name));
        }
    }
}

fn filtered_project_mcp(
    repo: &Path,
    cas_root: &Path,
    policy: &FactoryWorkerPolicy,
) -> std::io::Result<Vec<u8>> {
    let mut document: serde_json::Value = match std::fs::read(repo.join(".mcp.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|_| std::io::Error::other("invalid project MCP configuration"))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(error) => return Err(error),
    };
    let object = document
        .as_object_mut()
        .ok_or_else(|| std::io::Error::other("project MCP configuration must be an object"))?;
    let servers = object
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| std::io::Error::other("project MCP servers must be an object"))?;
    servers.retain(|name, _| !policy.denies_server(name));
    for server in servers.values_mut() {
        if let Some(env) = server
            .get_mut("env")
            .and_then(serde_json::Value::as_object_mut)
        {
            env.retain(|name, _| !policy.denies_env(name));
        }
    }
    // Strict scope must keep Cassy even when the operator registered it only
    // in a local/user scope. Identity/session metadata is inherited at spawn.
    servers.entry("cas").or_insert_with(|| {
        serde_json::json!({
            "command": "cas", "args": ["serve"], "env": {"CAS_ROOT": cas_root}
        })
    });
    serde_json::to_vec_pretty(&document).map_err(std::io::Error::other)
}

fn write_private(destination: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = destination
        .parent()
        .ok_or_else(|| std::io::Error::other("MCP path has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(std::io::Error::other)?
        .as_nanos();
    let temporary = parent.join(format!(".worker-mcp-{}-{nonce}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        // Rename replaces the private directory entry, never writes through
        // an old symlink into the supervisor's source configuration.
        std::fs::rename(&temporary, destination)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

/// Provisioning seam shared by worktree creation and worker launch.
/// With no configured denials the existing project-link policy remains.
pub fn provision_project_mcp(repo: &Path, worker_name: &str) -> std::io::Result<bool> {
    let cas_root = repo.join(".cas");
    let policy = load_worker_policy(Some(&cas_root))?;
    if policy.is_empty() {
        return Ok(false);
    }
    prepare_worker_mcp(&cas_root, worker_name, &policy)?;
    Ok(true)
}

/// Prepare an explicit private config for every worker. Existing worktree
/// `.mcp.json` files may be tracked, so no worker mode writes into the checkout.
pub(crate) fn prepare_worker_mcp(
    cas_root: &Path,
    name: &str,
    policy: &FactoryWorkerPolicy,
) -> std::io::Result<PathBuf> {
    let repo = cas_root
        .parent()
        .ok_or_else(|| std::io::Error::other("Cassy store has no project root"))?;
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        || name.is_empty()
    {
        return Err(std::io::Error::other(
            "invalid worker name for MCP configuration",
        ));
    }
    let destination = cas_root.join("worker-mcp").join(format!("{name}.json"));
    write_private(&destination, &filtered_project_mcp(repo, cas_root, policy)?)?;
    Ok(destination)
}
