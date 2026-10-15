//! Keep heavy worker builds outside the factory's interactive cgroup (GH #1213).
use std::path::Path;
use std::process::Command;

const GIB: u64 = 1024 * 1024 * 1024;
const INTERACTIVE_LOW: u64 = 256 * 1024 * 1024;

pub(crate) fn memory_high(configured_gib: u32, builders: usize, meminfo: &str) -> u64 {
    if configured_gib != 0 {
        return u64::from(configured_gib) * GIB;
    }
    let total = meminfo
        .lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            (fields.next()? == "MemTotal:").then_some(())?;
            let kib = fields.next()?.parse::<u64>().ok()?;
            (fields.next()? == "kB").then(|| kib.saturating_mul(1024))
        })
        .unwrap_or(8 * GIB);
    // Leave half the host's RAM to interactive processes and other workloads.
    // Bound very small/large hosts; 64 GiB with four builders yields 8 GiB each.
    (total / 2 / builders.max(1) as u64).clamp(GIB / 4, 16 * GIB)
}

/// Inject the executable, rather than invoking real systemd in tests. Scope
/// mode execs locally and retains cwd, environment and inherited target leases.
/// Wrap before installing pre_exec hooks; Command cannot clone those hooks.
pub(crate) fn scope_command(
    command: Command,
    runner: Option<&Path>,
    high: u64,
    marker: &Path,
) -> Command {
    let Some(runner) = runner else {
        eprintln!(
            "Worker build: systemd user scopes unavailable on this platform; running unscoped"
        );
        return command;
    };
    let mut scoped = Command::new("bash");
    scoped.args([
        "-c",
        r#"
runner=$1
high=$2
marker=$3
shift 3
"$runner" --user --scope --quiet -p "MemoryHigh=$high" -- bash -c '
marker=$1
shift
: > "$marker" || exit 125
exec "$@"
' worker-build-start "$marker" "$@"
status=$?
if [ -f "$marker" ]; then
    exit "$status"
fi
printf '%s\n' 'Worker build: systemd user scope unavailable; running unscoped' >&2
exec "$@"
"#,
        "worker-build-scope",
    ]);
    scoped
        .arg(runner)
        .arg(high.to_string())
        .arg(marker)
        .arg(command.get_program())
        .args(command.get_args());
    for (key, value) in command.get_envs() {
        if let Some(value) = value {
            scoped.env(key, value);
        } else {
            scoped.env_remove(key);
        }
    }
    if let Some(cwd) = command.get_current_dir() {
        scoped.current_dir(cwd);
    }
    scoped
}

/// The daemon and supervisor inherit the factory's interactive cgroup. Once
/// builds move to sibling systemd scopes, protect that interactive working set.
/// Never lower an operator's stronger protection or make startup depend on it.
pub(crate) fn protect_interactive_memory() {
    #[cfg(target_os = "linux")]
    {
        let result = std::fs::read_to_string("/proc/self/cgroup").and_then(|membership| {
            protect_low_at(
                Path::new("/sys/fs/cgroup"),
                &membership,
                |path, value| std::fs::write(path, value),
                |unit| {
                    use crate::bounded_process::{Deadline, run_command};
                    let budget = std::time::Duration::from_secs(1);
                    let mut command = Command::new("systemctl");
                    command
                        .args(["--user", "set-property", "--runtime", unit])
                        .arg(format!("MemoryLow={INTERACTIVE_LOW}"));
                    match run_command(&mut command, Deadline::after(budget), budget) {
                        Ok(output) if output.status.success() => Ok(()),
                        _ => Err(std::io::Error::other("systemd user MemoryLow unavailable")),
                    }
                },
            )
        });
        match result {
            Ok(()) => tracing::info!(
                "factory interactive cgroup has at least 256 MiB memory.low protection"
            ),
            Err(error) => {
                tracing::warn!(%error, "factory interactive memory.low unavailable; continuing without protection")
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    tracing::info!("factory interactive memory.low unavailable on this platform");
}

#[cfg(any(target_os = "linux", test))]
fn protect_low_at(
    root: &Path,
    membership: &str,
    write: impl FnOnce(&Path, &str) -> std::io::Result<()>,
    systemd: impl FnOnce(&str) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let relative = membership
        .lines()
        .find_map(|line| line.strip_prefix("0::/"))
        .filter(|path| {
            !path.is_empty()
                && Path::new(path)
                    .components()
                    .all(|part| matches!(part, std::path::Component::Normal(_)))
        })
        .ok_or_else(|| std::io::Error::other("no non-root unified interactive cgroup"))?;
    let path = root.join(relative).join("memory.low");
    let current = std::fs::read_to_string(&path)?;
    if current.trim() == "max"
        || current
            .trim()
            .parse::<u64>()
            .is_ok_and(|low| low >= INTERACTIVE_LOW)
    {
        return Ok(());
    }
    if let Err(error) = write(&path, &INTERACTIVE_LOW.to_string()) {
        // systemd owns non-delegated scope properties even when cgroupfs is
        // not user-writable. Restrict this fallback to the caller's own unit.
        let unit = Path::new(relative)
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| name.ends_with(".scope") || name.ends_with(".service"));
        return match unit {
            Some(unit) => systemd(unit),
            None => Err(error),
        };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn script_payload(directory: &Path) -> Command {
        let mut command = Command::new("bash");
        command
            .args([
                "-c",
                r#"printf '%s\n' "$CAS_192B_VALUE" '$CAS_192B_LITERAL' "$1" "${2}""#,
                "payload",
                "$CAS_192B_ARGUMENT",
                "${CAS_192B_BRACED}",
            ])
            .env("CAS_192B_VALUE", "child value with spaces")
            .current_dir(directory);
        command
    }

    #[cfg(unix)]
    fn assert_scoped_script_survives(runner: &Path, directory: &Path) {
        let marker = directory.join("started-$CAS_192B_MARKER");
        let output = scope_command(script_payload(directory), Some(runner), GIB, &marker)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "scope script failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(marker.is_file(), "the inner script must mark a real scoped launch");
        // pin: every dollar expression must reach Bash unchanged; only the
        // payload's unquoted child variable may expand, after scope admission.
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            "child value with spaces\n$CAS_192B_LITERAL\n$CAS_192B_ARGUMENT\n${CAS_192B_BRACED}\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn worker_memory_scope_preserves_scripts_with_systemd_expansion_cas_192b() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let runner = directory.path().join("systemd-run");
        // Model the external argv-expansion boundary, not shell evaluation:
        // systemd expands even dollar expressions inside shell quote characters.
        std::fs::write(&runner, r#"#!/usr/bin/python3
import os, re, sys
args = sys.argv[1:]
if args == ['--version']:
    print('systemd 259 (fixture)')
    sys.exit(0)
assert '--scope' in args
assert 'MemoryHigh=1073741824' in args
expand = '--expand-environment=no' not in args
args = args[args.index('--') + 1:]
if expand:
    def replace(match):
        if match[0] == '$$':
            return '$'
        return os.environ.get(match[1] or match[2], '')
    args = [re.sub(r'\$\$|\$\{([^}]+)\}|\$([A-Za-z_][A-Za-z_0-9]*)', replace, arg) for arg in args]
os.execvpe(args[0], args, os.environ)
"#).unwrap();
        std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_scoped_script_survives(&runner, directory.path());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn worker_memory_scope_preserves_scripts_with_real_systemd_cas_192b() {
        use crate::bounded_process::{Deadline, run_command};
        let budget = std::time::Duration::from_secs(2);
        let mut available = Command::new("systemd-run");
        available.args(["--user", "--scope", "--quiet", "--", "true"]);
        if !run_command(&mut available, Deadline::after(budget), budget)
            .is_ok_and(|output| output.status.success())
        {
            eprintln!("SKIP real systemd control: user scopes unavailable");
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        assert_scoped_script_survives(Path::new("systemd-run"), directory.path());
    }

    #[test]
    fn worker_memory_budget_reserves_ram_and_tracks_builder_cap() {
        let meminfo = "MemTotal:       67108864 kB\nMemAvailable: 100 kB\n";
        assert_eq!(memory_high(0, 4, meminfo), 8 * GIB);
        assert_eq!(memory_high(0, 8, meminfo), 4 * GIB);
        assert_eq!(memory_high(3, 8, meminfo), 3 * GIB);
        assert!(memory_high(0, 0, meminfo) <= 16 * GIB);
        assert_eq!(memory_high(0, 4, ""), GIB);
        assert!(memory_high(0, 256, "MemTotal: 100 kB") >= GIB / 4);
    }

    #[test]
    fn worker_memory_interactive_low_preserves_stronger_protection() {
        let root = tempfile::tempdir().unwrap();
        let scope = root.path().join("user.slice/factory.scope");
        std::fs::create_dir_all(&scope).unwrap();
        let low = scope.join("memory.low");
        std::fs::write(&low, "0").unwrap();
        let membership = "0::/user.slice/factory.scope\n";
        protect_low_at(
            root.path(),
            membership,
            |path, value| std::fs::write(path, value),
            |_| panic!("delegated file is writable"),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(&low)
                .unwrap()
                .parse::<u64>()
                .unwrap(),
            INTERACTIVE_LOW
        );
        for stronger in [(INTERACTIVE_LOW * 2).to_string(), "max".into()] {
            std::fs::write(&low, &stronger).unwrap();
            protect_low_at(
                root.path(),
                membership,
                |_, _| panic!("must not reduce protection"),
                |_| panic!("existing protection is enough"),
            )
            .unwrap();
            assert_eq!(std::fs::read_to_string(&low).unwrap(), stronger);
        }
        for unsafe_path in ["0::/", "0::/../factory.scope", "2:memory:/factory.scope"] {
            assert!(
                protect_low_at(
                    root.path(),
                    unsafe_path,
                    |_, _| panic!("invalid cgroup"),
                    |_| panic!("invalid cgroup")
                )
                .is_err()
            );
        }
    }

    #[test]
    fn worker_memory_interactive_low_uses_user_manager_when_not_delegated() {
        let root = tempfile::tempdir().unwrap();
        let scope = root.path().join("factory.scope");
        std::fs::create_dir(&scope).unwrap();
        let low = scope.join("memory.low");
        std::fs::write(&low, "0").unwrap();
        let mut requested = None;
        let denied =
            |_: &Path, _: &str| Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        protect_low_at(root.path(), "0::/factory.scope", denied, |unit| {
            requested = Some(unit.to_string());
            Ok(())
        })
        .unwrap();
        assert_eq!(requested.as_deref(), Some("factory.scope"));
        assert!(
            protect_low_at(root.path(), "0::/factory.scope", denied, |_| Err(
                std::io::Error::other("no user manager")
            ))
            .is_err()
        );
    }
}
