//! Task-created iOS simulators (cas-75a8, GH #1296).
//!
//! Workers create simulators for proofs and repros and name them for their
//! task (`cas-10e9-proof`, `cas-307f-repro`). Nothing deleted them, so 15
//! devices held about 35 GiB on one Mac. The task prefix of the device name is
//! the ownership record: it is how every observed device is named, and it
//! needs no separate registry that a crashed worker could fail to write.
//!
//! A device is reclaimable only when its owner is a task of this project that
//! is terminal (closed or cancelled) for at least [`SIMULATOR_TTL_HOURS`], and
//! the device is shut down. Devices of open tasks, of recently ended tasks,
//! booted devices and task-shaped names this project does not know are listed
//! but never deleted.

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Duration, Utc};

/// Hours after a task ends before its simulators may be deleted.
pub const SIMULATOR_TTL_HOURS: i64 = 24;

/// One simulator device as `xcrun simctl list devices --json` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimDevice {
    pub udid: String,
    pub name: String,
    pub state: String,
    pub runtime: String,
    /// `dataPathSize`, when this Xcode reports it.
    pub bytes: Option<u64>,
}

/// What gc does with a task-named device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SimVerdict {
    /// Owner task ended at least the TTL ago and the device is shut down.
    Reclaimable,
    /// Owner task is still open.
    OpenTask,
    /// Owner task ended inside the TTL.
    Recent,
    /// The device is not shut down.
    NotShutdown,
    /// The task id is not a task of this project.
    UnknownTask,
    /// cas-6162: another known project also has this task id, or ownership
    /// cannot be checked; the device is never deleted.
    Unproven,
}

impl SimVerdict {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Reclaimable => "reclaimable",
            Self::OpenTask => "kept: task open",
            Self::Recent => "kept: task ended under 24h ago",
            Self::NotShutdown => "kept: not shut down",
            Self::UnknownTask => "review only: not a task of this project",
            Self::Unproven => "kept: task id not provably this project's",
        }
    }
}

/// A task-named device and its verdict.
#[derive(Debug, Clone)]
pub struct TaskSimulator {
    pub device: SimDevice,
    pub task_id: String,
    pub verdict: SimVerdict,
}

/// The task id a device name starts with (`cas-<hex>` then `-` or the end).
pub fn owner_task(name: &str) -> Option<&str> {
    let rest = name.strip_prefix("cas-")?;
    let hex = rest.bytes().take_while(u8::is_ascii_hexdigit).count();
    let boundary = rest.as_bytes().get(hex).is_none_or(|byte| *byte == b'-');
    ((4..=16).contains(&hex) && boundary).then(|| &name[..4 + hex])
}

/// Parse `xcrun simctl list devices --json`.
pub fn parse_devices(json: &str) -> Result<Vec<SimDevice>> {
    let value: serde_json::Value =
        serde_json::from_str(json).context("simctl device list is not JSON")?;
    let runtimes = value
        .get("devices")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| anyhow!("simctl device list has no devices object"))?;
    let mut devices = Vec::new();
    for (runtime, list) in runtimes {
        for device in list.as_array().into_iter().flatten() {
            let field = |key: &str| device.get(key).and_then(serde_json::Value::as_str).map(str::to_string);
            let (Some(udid), Some(name)) = (field("udid"), field("name")) else {
                continue;
            };
            devices.push(SimDevice {
                udid,
                name,
                state: field("state").unwrap_or_default(),
                runtime: runtime.clone(),
                bytes: device.get("dataPathSize").and_then(serde_json::Value::as_u64),
            });
        }
    }
    devices.sort_by(|a, b| a.name.cmp(&b.name).then(a.udid.cmp(&b.udid)));
    Ok(devices)
}

/// Classify every task-named device against this project's tasks. A device
/// is reclaimable only when no other known project holds the same task id
/// (cas-6162): the name carries an id, not a project.
pub fn classify(
    devices: &[SimDevice],
    tasks: &[cas_types::Task],
    others: &dyn crate::task_ownership::OtherProjects,
    now: DateTime<Utc>,
) -> Vec<TaskSimulator> {
    let cutoff = now - Duration::hours(SIMULATOR_TTL_HOURS);
    devices
        .iter()
        .filter_map(|device| {
            let task_id = owner_task(&device.name)?.to_string();
            let verdict = match tasks.iter().find(|task| task.id == task_id) {
                None => SimVerdict::UnknownTask,
                Some(task) if !task.is_terminal() => SimVerdict::OpenTask,
                Some(task) if task.closed_at.unwrap_or(task.updated_at) > cutoff => SimVerdict::Recent,
                Some(_) if device.state != "Shutdown" => SimVerdict::NotShutdown,
                Some(_) => match others.holders(&task_id) {
                    Ok(holders) if holders.is_empty() => SimVerdict::Reclaimable,
                    _ => SimVerdict::Unproven,
                },
            };
            Some(TaskSimulator { device: device.clone(), task_id, verdict })
        })
        .collect()
}

/// The longest one `xcrun simctl` call may take before gc reports the
/// simulators as unavailable (cas-6162).
pub const SIMCTL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Runs `xcrun simctl`; tests substitute a fake executable.
pub struct Simctl {
    xcrun: PathBuf,
    timeout: std::time::Duration,
}

impl Default for Simctl {
    fn default() -> Self {
        Self { xcrun: PathBuf::from("xcrun"), timeout: SIMCTL_TIMEOUT }
    }
}

impl Simctl {
    pub fn with_binary(xcrun: impl Into<PathBuf>) -> Self {
        Self { xcrun: xcrun.into(), timeout: SIMCTL_TIMEOUT }
    }

    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// cas-6162: bounded, in its own process group, so a timeout or a failed
    /// wait kills simctl and anything it started (via `bounded_process`).
    fn run(&self, args: &[&str]) -> Result<String> {
        let mut command = std::process::Command::new(&self.xcrun);
        command.arg("simctl").args(args);
        let output = crate::bounded_process::run_command(
            &mut command,
            crate::bounded_process::Deadline::after(self.timeout),
            self.timeout,
        )
        .map_err(|error| match error {
            crate::bounded_process::BoundedCommandError::TimedOut => anyhow!(
                "xcrun simctl {} timed out after {}s",
                args.join(" "),
                self.timeout.as_secs_f64()
            ),
            crate::bounded_process::BoundedCommandError::Io => {
                anyhow!("could not run {} simctl {}", self.xcrun.display(), args.join(" "))
            }
        })?;
        if !output.status.success() {
            return Err(anyhow!(
                "xcrun simctl {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    pub fn devices(&self) -> Result<Vec<SimDevice>> {
        parse_devices(&self.run(&["list", "devices", "--json"])?)
    }

    pub fn delete(&self, udid: &str) -> Result<()> {
        self.run(&["delete", udid]).map(|_| ())
    }
}

/// The task simulators on this host, or why they could not be listed.
pub fn inventory(
    simctl: &Simctl,
    tasks: &[cas_types::Task],
    others: &dyn crate::task_ownership::OtherProjects,
    now: DateTime<Utc>,
) -> Result<Vec<TaskSimulator>> {
    Ok(classify(&simctl.devices()?, tasks, others, now))
}

/// Delete every reclaimable device. Returns (deleted, failures).
pub fn reclaim(simctl: &Simctl, simulators: &[TaskSimulator]) -> (Vec<String>, Vec<String>) {
    let mut deleted = Vec::new();
    let mut failed = Vec::new();
    for simulator in simulators.iter().filter(|sim| sim.verdict == SimVerdict::Reclaimable) {
        match simctl.delete(&simulator.device.udid) {
            Ok(()) => deleted.push(simulator.device.name.clone()),
            Err(error) => failed.push(format!("{}: {error:#}", simulator.device.name)),
        }
    }
    (deleted, failed)
}

/// Human report lines shared by gc_report, gc_cleanup and `cas artifact prune`.
pub fn render(result: &Result<Vec<TaskSimulator>>) -> String {
    match result {
        Err(error) => format!("\nTask simulators: unavailable ({error:#})\n"),
        Ok(simulators) => {
            let reclaimable = simulators.iter().filter(|sim| sim.verdict == SimVerdict::Reclaimable).count();
            let mut out = format!(
                "\nTask simulators: {} task-named, {reclaimable} reclaimable (task ended >=24h, shut down)\n",
                simulators.len()
            );
            for sim in simulators {
                let size = sim.device.bytes.map_or_else(|| "size unknown".to_string(), |bytes| format!("{bytes} bytes"));
                out.push_str(&format!(
                    "  - {} ({}) {} {}: {}\n",
                    sim.device.name, sim.device.udid, sim.device.state, size, sim.verdict.label()
                ));
            }
            out
        }
    }
}

/// Whether this host can have simulators at all.
pub fn supported() -> bool {
    cfg!(target_os = "macos")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// cas-75a8 (GH #1296): only a shut-down device of a task that ended at
    /// least 24h ago is deleted; open, recent, booted, unknown-task and
    /// ordinary devices stay.
    #[test]
    #[cfg(unix)]
    fn gc_deletes_only_shut_down_simulators_of_long_ended_tasks_cas_75a8() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("deleted.log");
        let devices = r#"{"devices":{"com.apple.CoreSimulator.SimRuntime.iOS-18-0":[
            {"udid":"U-OLD","name":"cas-10e9-proof","state":"Shutdown","dataPathSize":4100000000},
            {"udid":"U-OPEN","name":"cas-307f-repro","state":"Shutdown"},
            {"udid":"U-RECENT","name":"cas-4aa0-repro","state":"Shutdown"},
            {"udid":"U-BOOTED","name":"cas-976c-alarm","state":"Booted"},
            {"udid":"U-FOREIGN","name":"cas-ffff-proof","state":"Shutdown"},
            {"udid":"U-PLAIN","name":"iPhone 16","state":"Shutdown"}]}}"#;
        let xcrun = dir.path().join("xcrun");
        std::fs::write(
            &xcrun,
            format!(
                "#!/bin/sh\ncase \"$2\" in\nlist) cat <<'JSON'\n{devices}\nJSON\n;;\ndelete) echo \"$3\" >> '{}';;\nesac\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&xcrun, std::fs::Permissions::from_mode(0o700)).unwrap();
        let now = Utc::now();
        let task = |id: &str, ended_hours: Option<i64>| {
            let mut task = cas_types::Task::new(id.into(), id.into());
            if let Some(hours) = ended_hours {
                task.status = cas_types::TaskStatus::Closed;
                task.closed_at = Some(now - Duration::hours(hours));
            }
            task
        };
        let tasks = vec![
            task("cas-10e9", Some(48)),
            task("cas-307f", None),
            task("cas-4aa0", Some(2)),
            task("cas-976c", Some(48)),
        ];
        let simctl = Simctl::with_binary(&xcrun);
        let none = crate::task_ownership::KnownProjects::from_roots(dir.path(), Vec::new());
        let simulators = inventory(&simctl, &tasks, &none, now).unwrap();
        let verdicts: Vec<_> = simulators.iter().map(|sim| (sim.device.name.as_str(), sim.verdict.clone())).collect();
        assert_eq!(
            verdicts,
            vec![
                ("cas-10e9-proof", SimVerdict::Reclaimable),
                ("cas-307f-repro", SimVerdict::OpenTask),
                ("cas-4aa0-repro", SimVerdict::Recent),
                ("cas-976c-alarm", SimVerdict::NotShutdown),
                ("cas-ffff-proof", SimVerdict::UnknownTask),
            ]
        );
        let (deleted, failed) = reclaim(&simctl, &simulators);
        assert_eq!((deleted, failed), (vec!["cas-10e9-proof".to_string()], Vec::<String>::new()));
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "U-OLD\n");
        assert!(render(&Ok(simulators)).contains("cas-10e9-proof (U-OLD) Shutdown 4100000000 bytes: reclaimable"));
    }

    /// Another project holding the same task ids.
    struct OtherHolds(&'static [&'static str]);
    impl crate::task_ownership::OtherProjects for OtherHolds {
        fn holders(&self, task_id: &str) -> Result<Vec<String>, String> {
            Ok(if self.0.contains(&task_id) { vec!["/projects/other".into()] } else { Vec::new() })
        }
    }

    /// cas-6162: a simulator whose task id another known project also has is
    /// never selected, even when this project's task with that id is long
    /// closed; an unreadable registry keeps it too.
    #[test]
    fn a_same_id_simulator_of_another_project_is_never_reclaimable_cas_6162() {
        let now = Utc::now();
        let mut closed = cas_types::Task::new("cas-10e9".into(), "closed here".into());
        closed.status = cas_types::TaskStatus::Closed;
        closed.closed_at = Some(now - Duration::hours(48));
        let device = SimDevice {
            udid: "U-1".into(),
            name: "cas-10e9-proof".into(),
            state: "Shutdown".into(),
            runtime: "iOS".into(),
            bytes: None,
        };
        let verdict = |others: &dyn crate::task_ownership::OtherProjects| {
            classify(std::slice::from_ref(&device), std::slice::from_ref(&closed), others, now)[0].verdict.clone()
        };
        assert_eq!(verdict(&OtherHolds(&[])), SimVerdict::Reclaimable);
        assert_eq!(verdict(&OtherHolds(&["cas-10e9"])), SimVerdict::Unproven);
        struct Unreadable;
        impl crate::task_ownership::OtherProjects for Unreadable {
            fn holders(&self, _: &str) -> Result<Vec<String>, String> {
                Err("registry unreadable".into())
            }
        }
        assert_eq!(verdict(&Unreadable), SimVerdict::Unproven);
    }

    /// cas-6162: a hung `xcrun simctl` returns within the bound, and the
    /// report says the simulators are unavailable.
    #[test]
    #[cfg(unix)]
    fn a_hung_simctl_returns_within_its_bound_cas_6162() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let xcrun = dir.path().join("xcrun");
        // A non-exec wrapper: the sleep is a grandchild that must die with the
        // process group, or the bounded call would leak it.
        let pidfile = dir.path().join("grandchild.pid");
        std::fs::write(&xcrun, format!("#!/bin/sh\nsleep 30 &\necho $! > '{}'\nwait\n", pidfile.display())).unwrap();
        std::fs::set_permissions(&xcrun, std::fs::Permissions::from_mode(0o700)).unwrap();
        let simctl = Simctl::with_binary(&xcrun).with_timeout(std::time::Duration::from_millis(300));
        let none = crate::task_ownership::KnownProjects::from_roots(dir.path(), Vec::new());
        let started = std::time::Instant::now();
        let result = inventory(&simctl, &[], &none, Utc::now());
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
        let report = render(&result);
        assert!(report.contains("Task simulators: unavailable") && report.contains("timed out"), "{report}");
        let pid: i32 = std::fs::read_to_string(&pidfile).unwrap().trim().parse().unwrap();
        let alive = |pid: i32| match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            // Linux: a reaped or zombie process is dead.
            Ok(stat) => !stat.rsplit(')').next().unwrap_or("").trim_start().starts_with('Z'),
            Err(_) if std::path::Path::new("/proc/self/stat").exists() => false,
            Err(_) => unsafe { libc::kill(pid, 0) == 0 },
        };
        let gone_by = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while alive(pid) && std::time::Instant::now() < gone_by {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!alive(pid), "the grandchild sleep {pid} must die with the process group");
    }
}
