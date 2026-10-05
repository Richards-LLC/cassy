//! The same owner/lease protocol powers proof startup and factory GC.
use super::*;

pub(crate) fn render(cas_root: &Path, clean: bool) -> String {
    let Some(repo) = cas_root.parent() else {
        return "\nRelease scratch: unavailable (no project root; none deleted)\n".into();
    };
    let result = std::process::Command::new("python3")
        .arg("-c")
        .arg(include_str!("../../../scripts/release_scratch.py"))
        .arg("--repo")
        .arg(repo)
        .arg("--cache")
        .arg(cas_root.join("merge-sweeps/assembly-target"))
        .arg(if clean { "clean" } else { "report" })
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
