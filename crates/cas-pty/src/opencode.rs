//! OpenCode account-root layout helpers.
//!
//! OpenCode's `OPENCODE_CONFIG_DIR` only relocates the project configuration.
//! An isolated Cassy worker therefore scopes all four XDG roots under one
//! account root.  These helpers only resolve and render environment entries;
//! they deliberately do not create directories or copy credentials.

use std::path::{Path, PathBuf};

/// Environment variable carrying the selected OpenCode account root.
pub const ACCOUNT_ROOT_ENV: &str = "CAS_OPENCODE_ACCOUNT_DIR";

/// XDG roots OpenCode uses for config, account data, state, and cache.
pub const XDG_ROOTS: [(&str, &str); 4] = [
    ("XDG_CONFIG_HOME", "config"),
    ("XDG_DATA_HOME", "data"),
    ("XDG_STATE_HOME", "state"),
    ("XDG_CACHE_HOME", "cache"),
];

/// Marker used by the PTY layer to distinguish an explicit account selection
/// from ordinary environment inheritance.
pub const ACCOUNT_ROOT_SOURCE_ENV: &str = "CAS_FACTORY_OPENCODE_ACCOUNT_DIR_SOURCE";

/// Resolve a user-provided account root against the process home directory.
pub fn resolve_account_root(raw: &str) -> Result<PathBuf, String> {
    resolve_account_root_from(raw, dirs::home_dir().as_deref())
}

/// Testable form of [`resolve_account_root`] with an explicit home directory.
pub fn resolve_account_root_from(raw: &str, home: Option<&Path>) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("config_dir is empty; expected an OpenCode account root".to_string());
    }

    let path = if trimmed == "~" {
        home.map(Path::to_path_buf).ok_or_else(|| {
            "config_dir uses `~`, but the home directory could not be resolved".to_string()
        })?
    } else if let Some(suffix) = trimmed.strip_prefix("~/") {
        home.map(|home| home.join(suffix)).ok_or_else(|| {
            "config_dir uses `~`, but the home directory could not be resolved".to_string()
        })?
    } else if trimmed.starts_with('~') {
        return Err(format!(
            "config_dir has unsupported tilde form {trimmed:?}; use `~` or `~/path`"
        ));
    } else {
        PathBuf::from(trimmed)
    };

    if path.exists() && !path.is_dir() {
        return Err(format!(
            "config_dir preflight failed: OpenCode account root {} exists but is not a directory",
            path.display()
        ));
    }
    Ok(path)
}

/// Render the environment for one isolated OpenCode worker account.
///
/// The function is side-effect free.  In particular, it does not create the
/// four directories and never reads or writes account credentials.
pub fn account_root_env(
    raw: &str,
    source: Option<&str>,
    home: Option<&Path>,
) -> Result<Vec<(String, String)>, String> {
    let root = resolve_account_root_from(raw, home)?;
    let mut env = vec![(ACCOUNT_ROOT_ENV.to_string(), root.display().to_string())];
    for (variable, child) in XDG_ROOTS {
        env.push((variable.to_string(), root.join(child).display().to_string()));
    }
    if let Some(source) = source.filter(|source| !source.trim().is_empty()) {
        env.push((ACCOUNT_ROOT_SOURCE_ENV.to_string(), source.to_string()));
    }
    Ok(env)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "cas-opencode-account-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn value<'a>(env: &'a [(String, String)], key: &str) -> &'a str {
        env.iter()
            .find_map(|(candidate, value)| (candidate == key).then_some(value.as_str()))
            .unwrap_or_else(|| panic!("missing {key}"))
    }

    #[test]
    fn tilde_root_expands_against_supplied_home_without_writing() {
        let home = TempRoot::new();
        let env = account_root_env("~/opencode-one", Some("test"), Some(home.path())).unwrap();
        let root = home.path().join("opencode-one");

        assert_eq!(value(&env, ACCOUNT_ROOT_ENV), root.to_string_lossy());
        assert_eq!(
            value(&env, "XDG_CONFIG_HOME"),
            root.join("config").to_string_lossy()
        );
        assert_eq!(
            value(&env, "XDG_DATA_HOME"),
            root.join("data").to_string_lossy()
        );
        assert_eq!(
            value(&env, "XDG_STATE_HOME"),
            root.join("state").to_string_lossy()
        );
        assert_eq!(
            value(&env, "XDG_CACHE_HOME"),
            root.join("cache").to_string_lossy()
        );
        assert_eq!(value(&env, ACCOUNT_ROOT_SOURCE_ENV), "test");
        assert!(
            !root.exists(),
            "layout resolution must not create account state"
        );
    }

    #[test]
    fn two_account_roots_have_disjoint_xdg_layouts() {
        let home = TempRoot::new();
        let first = account_root_env("~/accounts/one", None, Some(home.path())).unwrap();
        let second = account_root_env("~/accounts/two", None, Some(home.path())).unwrap();

        for variable in [
            ACCOUNT_ROOT_ENV,
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_STATE_HOME",
            "XDG_CACHE_HOME",
        ] {
            assert_ne!(
                value(&first, variable),
                value(&second, variable),
                "{variable}"
            );
        }
    }

    #[test]
    fn blank_invalid_tilde_and_file_roots_are_rejected() {
        let home = TempRoot::new();
        assert!(resolve_account_root_from("  ", Some(home.path())).is_err());
        assert!(resolve_account_root_from("~other/account", Some(home.path())).is_err());

        let file = home.path().join("not-a-directory");
        std::fs::write(&file, b"sentinel").unwrap();
        let error =
            resolve_account_root_from(file.to_str().unwrap(), Some(home.path())).unwrap_err();
        assert!(error.contains("not a directory"), "{error}");
        assert_eq!(std::fs::read(file).unwrap(), b"sentinel");
    }
}
