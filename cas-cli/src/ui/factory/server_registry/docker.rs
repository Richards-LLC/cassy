//! Track a direct `docker run` by its immutable daemon container ID.

use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DockerRun {
    executable: PathBuf,
    cid_file: PathBuf,
}

/// Preserve shell expansion in the arguments; only inject the CID-file option
/// after a direct Docker `run`. Shell wrappers are not inferred as Docker runs.
pub(super) fn prepare(
    command: &str,
    cwd: &Path,
    registry: &Path,
) -> io::Result<(String, Option<DockerRun>)> {
    let direct = command
        .trim()
        .strip_prefix("exec ")
        .unwrap_or(command.trim())
        .trim_start();
    let Some((program, rest)) = direct.split_once(char::is_whitespace) else {
        return Ok((command.to_string(), None));
    };
    if Path::new(program).file_name().and_then(|s| s.to_str()) != Some("docker") {
        return Ok((command.to_string(), None));
    }
    let rest = rest.trim_start();
    let Some(args) = rest
        .strip_prefix("run")
        .filter(|s| s.starts_with(char::is_whitespace))
    else {
        return Ok((command.to_string(), None));
    };
    if args
        .split_whitespace()
        .any(|arg| arg == "--cidfile" || arg.starts_with("--cidfile="))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Cassy manages --cidfile for direct docker run commands; omit that option",
        ));
    }
    let executable = if program.contains('/') {
        let path = Path::new(program);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        }
    } else {
        PathBuf::from(program)
    };
    let cid_file = registry.join(format!("{}.cid", uuid::Uuid::new_v4()));
    let quoted = cid_file.to_string_lossy().replace('\'', "'\\''");
    Ok((
        format!("{program} run --cidfile '{quoted}'{args}"),
        Some(DockerRun {
            executable,
            cid_file,
        }),
    ))
}

impl DockerRun {
    fn command(&self, cwd: &Path, args: &[&str]) -> io::Result<std::process::Output> {
        let mut child = Command::new(&self.executable)
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if child.try_wait()?.is_some() {
                return child.wait_with_output();
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Docker lifecycle command exceeded 10 seconds",
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    pub(super) fn stop(&self, cwd: &Path) -> io::Result<()> {
        // A just-started client may still be contacting the daemon. Refuse to
        // claim success until its CID exists; retries use the same durable file.
        let deadline = std::time::Instant::now() + PID_PUBLISH_TIMEOUT;
        let id = loop {
            match fs::read_to_string(&self.cid_file) {
                Ok(id) => {
                    let id = id.trim().to_string();
                    if id.is_empty() && std::time::Instant::now() < deadline {
                        std::thread::sleep(std::time::Duration::from_millis(20));
                        continue;
                    }
                    if id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()) {
                        break id;
                    }
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Docker CID file does not contain a full container ID",
                    ));
                }
                Err(error)
                    if error.kind() == io::ErrorKind::NotFound
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(error) => {
                    return Err(io::Error::new(
                        error.kind(),
                        format!("Docker run has no readable container ID: {error}"),
                    ));
                }
            }
        };
        let stopped = self.command(cwd, &["stop", "--time=2", &id])?;
        let inspected = self.command(cwd, &["inspect", "--format={{.State.Running}}", &id])?;
        let stderr = String::from_utf8_lossy(&inspected.stderr).to_ascii_lowercase();
        let removed = !inspected.status.success()
            && (stderr.contains("no such object:") || stderr.contains("no such container:"));
        if removed
            || (inspected.status.success()
                && String::from_utf8_lossy(&inspected.stdout).trim() == "false")
        {
            return Ok(());
        }
        Err(io::Error::other(format!(
            "Docker container {id} was not confirmed stopped (stop {}, inspect {}): {}",
            stopped.status,
            inspected.status,
            String::from_utf8_lossy(&inspected.stderr).trim()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cas_9723_docker_command_preserves_shell_arguments() {
        let temp = tempfile::tempdir().unwrap();
        let original = "exec docker run --rm --name 'quoted name' -e KEY=$VALUE image";
        let (command, docker) = prepare(original, temp.path(), temp.path()).unwrap();
        let docker = docker.unwrap();
        assert!(command.starts_with("docker run --cidfile '"));
        assert!(command.ends_with(" --rm --name 'quoted name' -e KEY=$VALUE image"));
        assert!(docker.cid_file.starts_with(temp.path()));
        let original = "npm run dev && docker run image";
        let (command, docker) = prepare(original, temp.path(), temp.path()).unwrap();
        assert_eq!(command, original);
        assert!(
            docker.is_none(),
            "shell wrappers must not acquire unrelated container ownership"
        );
        assert!(
            prepare(
                "docker run --cidfile=/tmp/other image",
                temp.path(),
                temp.path()
            )
            .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn cas_9723_docker_stop_failure_never_claims_success() {
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("docker");
        crate::test_paths::warm_stub(
            &executable,
            "#!/bin/sh\ncase \"$1\" in\nstop) exit 1 ;;\ninspect) echo true ;;\nesac\n",
        );
        let cid_file = temp.path().join("container.cid");
        fs::write(&cid_file, format!("{:064}", 1)).unwrap();
        let docker = DockerRun {
            executable,
            cid_file,
        };
        let error = docker.stop(temp.path()).unwrap_err();
        assert!(
            error.to_string().contains("not confirmed stopped"),
            "{error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn cas_9723_docker_rm_and_already_stopped_are_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("docker");
        crate::test_paths::warm_stub(
            &executable,
            "#!/bin/sh\ncase \"$1\" in\nstop) exit 1 ;;\ninspect) echo 'Error: No such object: removed-id' >&2; exit 1 ;;\nesac\n",
        );
        let cid_file = temp.path().join("container.cid");
        fs::write(&cid_file, format!("{:064}", 1)).unwrap();
        let docker = DockerRun {
            executable,
            cid_file,
        };
        docker.stop(temp.path()).unwrap();
        docker.stop(temp.path()).unwrap();
    }
}
