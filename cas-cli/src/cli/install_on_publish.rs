//! Keep an installed install-on-publish agent current after `cas update`.
//!
//! `install-on-publish.sh --install` copies the script (and on Linux the
//! systemd units) once; nothing refreshed those copies, so hosts kept running
//! an old installer. The release embeds the files and rewrites only the ones
//! already installed that differ. A host without the agent is untouched.
use std::path::{Path, PathBuf};

const SCRIPT: &str = include_str!("../../../scripts/install-on-publish.sh");
const SERVICE: &str = include_str!("../../../ops/systemd/cassy-install-on-publish.service");
const TIMER: &str = include_str!("../../../ops/systemd/cassy-install-on-publish.timer");

/// Rewrite each installed, differing file; returns the paths it changed.
fn refresh_in(home: &Path, units_dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let files = [
        (home.join(".local/libexec/cassy/install-on-publish.sh"), SCRIPT, 0o755),
        (units_dir.join("cassy-install-on-publish.service"), SERVICE, 0o644),
        (units_dir.join("cassy-install-on-publish.timer"), TIMER, 0o644),
    ];
    let mut changed = Vec::new();
    for (path, content, mode) in files {
        match std::fs::read_to_string(&path) {
            Ok(current) if current != content => {}
            _ => continue, // not installed, or already current
        }
        let tmp = path.with_extension("refresh.tmp");
        std::fs::write(&tmp, content)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
        }
        #[cfg(not(unix))]
        let _ = mode;
        std::fs::rename(&tmp, &path)?;
        changed.push(path);
    }
    Ok(changed)
}

/// Best effort: a failure here never fails the update.
pub fn refresh_installed() {
    let Some(home) = dirs::home_dir() else { return };
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    match refresh_in(&home, &config.join("systemd/user")) {
        Ok(changed) if changed.iter().any(|p| p.extension().is_some_and(|e| e != "sh")) => {
            let _ = std::process::Command::new("systemctl")
                .args(["--user", "daemon-reload"])
                .status();
        }
        Ok(_) => {}
        Err(error) => eprintln!("install-on-publish refresh skipped: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refreshes_only_what_is_installed() {
        let dir = tempfile::tempdir().unwrap();
        let (home, units) = (dir.path(), dir.path().join("units"));
        // No agent installed: nothing is created.
        assert!(refresh_in(home, &units).unwrap().is_empty());
        assert!(!home.join(".local").exists());
        // An old script and service are rewritten; the absent timer is not.
        let script = home.join(".local/libexec/cassy/install-on-publish.sh");
        let service = units.join("cassy-install-on-publish.service");
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&units).unwrap();
        std::fs::write(&script, "old").unwrap();
        std::fs::write(&service, "old").unwrap();
        assert_eq!(refresh_in(home, &units).unwrap().len(), 2);
        assert_eq!(std::fs::read_to_string(&script).unwrap(), SCRIPT);
        assert_eq!(std::fs::read_to_string(&service).unwrap(), SERVICE);
        assert!(!units.join("cassy-install-on-publish.timer").exists());
        // Current files are left alone.
        assert!(refresh_in(home, &units).unwrap().is_empty());
    }
}
