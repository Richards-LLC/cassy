//! The same owner/lease protocol powers proof startup and factory GC.
use super::*;

pub(crate) fn render(cas_root: &Path, clean: bool) -> String {
    let Some(repo) = cas_root.parent() else {
        return "\nRelease scratch: unavailable (no project root; none deleted)\n".into();
    };
    let result = std::process::Command::new("python3")
        .arg("-c")
        .arg(include_str!("../../../scripts/release_scratch.py"))
        .args(release_scratch_args(cas_root, repo, clean))
        .output();
    match result {
        Ok(output) if output.status.success() => {
            let raw = String::from_utf8_lossy(&output.stdout);
            match serde_json::from_str::<serde_json::Value>(&raw) {
                Ok(value) => format!(
                    "\nRelease scratch: {} bytes reclaimable\nReclaimed: {} bytes; retained: {} bytes\nRELEASE_SCRATCH_STATUS_JSON={}\n",
                    value["reclaimable_bytes"],
                    value["reclaimed_bytes"],
                    value["retained_bytes"],
                    value,
                ),
                Err(error) => format!(
                    "\nRelease scratch: invalid inventory (none claimed deleted): {error}\n"
                ),
            }
        }
        Ok(output) => format!(
            "\nRelease scratch: unavailable (fail-closed): {}\n",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
        Err(error) => {
            format!("\nRelease scratch: unavailable (fail-closed; none deleted): {error}\n")
        }
    }
}

/// Arguments for release_scratch.py. The legacy `assembly-target` inventory
/// stays in-tree, where old caches were left. With `factory.scratch_root` set,
/// the scan covers the relocated release scratch base, which holds every
/// assembly clone and its target (cas-baa8).
fn release_scratch_args(cas_root: &Path, repo: &Path, clean: bool) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = vec![
        "--repo".into(),
        repo.into(),
        "--cache".into(),
        cas_root.join("merge-sweeps/assembly-target").into(),
    ];
    args.push(if clean { "clean" } else { "report" }.into());
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_root_release_scratch_report_scans_the_relocated_base() {
        let dir = tempfile::tempdir().unwrap();
        let cas = dir.path().join(".cas");
        std::fs::create_dir_all(&cas).unwrap();
        let unset = release_scratch_args(&cas, dir.path(), false);
        assert!(!unset.iter().any(|arg| arg == "--base"), "{unset:?}");
        let scratch = dir.path().join("rewind");
        std::fs::write(
            cas.join("config.toml"),
            format!("[factory]\nscratch_root = {:?}\n", scratch.display().to_string()),
        )
        .unwrap();
        let set = release_scratch_args(&cas, dir.path(), true);
        let base = scratch
            .join(cas_types::factory_scratch::project_key(&cas))
            .join("release-gate");
        let at = set.iter().position(|arg| arg == "--base").expect("--base passed");
        assert_eq!(std::path::Path::new(&set[at + 1]), base);
        assert_eq!(set.last().unwrap(), "clean");
        // The legacy in-tree inventory is still reported where it was left.
        assert!(set.iter().any(|arg| std::path::Path::new(arg) == cas.join("merge-sweeps/assembly-target")));
    }
}
