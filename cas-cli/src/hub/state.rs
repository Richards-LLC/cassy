use std::fs::{self, File};
use std::path::{Component, Path};

use anyhow::{Result, ensure};

/// Create a private hub state directory without following symlinks.
///
/// Missing path components are created owner-only. Existing ancestors are
/// preserved, but the closest existing ancestor must be an owner-controlled,
/// writable directory and the final directory must already satisfy the hub's
/// 0700/owner contract.
pub(crate) fn ensure_private_dir(path: &Path) -> Result<()> {
    ensure!(path.is_absolute(), "hub state path must be absolute");
    let missing_components = closest_existing_ancestor(path)?;

    #[cfg(unix)]
    {
        ensure_private_dir_unix(path, missing_components)
    }
    #[cfg(not(unix))]
    {
        ensure_private_dir_portable(path, missing_components)
    }
}

/// Return how many trailing components are absent while validating the
/// closest existing ancestor. `symlink_metadata` deliberately rejects a
/// symlink at that boundary instead of following it.
fn closest_existing_ancestor(path: &Path) -> Result<usize> {
    let mut candidate = path;
    let mut missing = 0;
    loop {
        match fs::symlink_metadata(candidate) {
            Ok(metadata) => {
                ensure!(
                    metadata.file_type().is_dir() && !metadata.file_type().is_symlink(),
                    "hub state hierarchy contains a symlink or non-directory"
                );
                #[cfg(unix)]
                validate_owner_controlled_ancestor(&metadata)?;
                return Ok(missing);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing += 1;
                candidate = candidate
                    .parent()
                    .ok_or_else(|| anyhow::anyhow!("hub state hierarchy has no directory root"))?;
            }
            Err(error) => return Err(sanitized_state_error(&error)),
        }
    }
}

#[cfg(unix)]
fn validate_owner_controlled_ancestor(metadata: &fs::Metadata) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    ensure!(
        metadata.uid() == unsafe { libc::geteuid() },
        "hub state hierarchy has the wrong owner"
    );
    ensure!(
        metadata.mode() & 0o300 == 0o300,
        "hub state hierarchy is not owner-writable"
    );
    Ok(())
}

#[cfg(unix)]
fn ensure_private_dir_unix(path: &Path, missing_components: usize) -> Result<()> {
    use std::ffi::{CString, OsStr};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    fn component_name(component: &OsStr) -> Result<CString> {
        CString::new(component.as_bytes())
            .map_err(|_| anyhow::anyhow!("hub state hierarchy contains an invalid component"))
    }

    fn open_child(parent: &File, name: &CString) -> std::io::Result<File> {
        let descriptor = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if descriptor == -1 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(unsafe { File::from_raw_fd(descriptor) })
        }
    }

    fn create_child(parent: &File, name: &CString) -> std::io::Result<()> {
        let result = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) };
        if result == -1 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    let components = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(Ok(name)),
            Component::RootDir | Component::CurDir => None,
            Component::ParentDir | Component::Prefix(_) => {
                Some(Err(anyhow::anyhow!("hub state path is not normalized")))
            }
        })
        .collect::<Result<Vec<_>>>()?;
    ensure!(!components.is_empty(), "hub state path has no directory");
    let ancestor_depth = components.len().saturating_sub(missing_components);
    let mut current = File::open("/").map_err(|error| sanitized_state_error(&error))?;

    for (index, component) in components.iter().enumerate() {
        let name = component_name(component)?;
        let mut created = false;
        let child = loop {
            match open_child(&current, &name) {
                Ok(child) => break child,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    match create_child(&current, &name) {
                        Ok(()) => created = true,
                        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                        Err(error) => return Err(sanitized_state_error(&error)),
                    }
                }
                Err(error) => return Err(sanitized_state_error(&error)),
            }
        };
        if created && unsafe { libc::fchmod(child.as_raw_fd(), 0o700) } == -1 {
            return Err(sanitized_state_error(&std::io::Error::last_os_error()));
        }
        let depth = index + 1;
        let metadata = child
            .metadata()
            .map_err(|error| sanitized_state_error(&error))?;
        if depth == ancestor_depth {
            validate_owner_controlled_ancestor(&metadata)?;
        }
        if created || depth == components.len() {
            validate_private_directory(&metadata)?;
        }
        current = child;
    }
    Ok(())
}

#[cfg(unix)]
fn validate_private_directory(metadata: &fs::Metadata) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    ensure!(
        metadata.uid() == unsafe { libc::geteuid() },
        "hub state directory has the wrong owner"
    );
    ensure!(
        metadata.mode() & 0o777 == 0o700,
        "hub state directory must have mode 0700"
    );
    Ok(())
}

#[cfg(not(unix))]
fn ensure_private_dir_portable(path: &Path, missing_components: usize) -> Result<()> {
    let mut missing = Vec::with_capacity(missing_components);
    let mut candidate = path;
    for _ in 0..missing_components {
        missing.push(candidate.to_path_buf());
        candidate = candidate
            .parent()
            .ok_or_else(|| anyhow::anyhow!("hub state hierarchy has no directory root"))?;
    }
    for component in missing.into_iter().rev() {
        match fs::create_dir(&component) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(sanitized_state_error(&error)),
        }
        let metadata =
            fs::symlink_metadata(&component).map_err(|error| sanitized_state_error(&error))?;
        ensure!(
            metadata.file_type().is_dir() && !metadata.file_type().is_symlink(),
            "hub state hierarchy contains a symlink or non-directory"
        );
    }
    Ok(())
}

fn sanitized_state_error(error: &std::io::Error) -> anyhow::Error {
    if matches!(
        error.kind(),
        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem
    ) {
        anyhow::anyhow!("hub state hierarchy is not accessible")
    } else if error.kind() == std::io::ErrorKind::NotADirectory
        || matches!(error.raw_os_error(), Some(libc::ELOOP))
    {
        anyhow::anyhow!("hub state hierarchy contains a symlink or non-directory")
    } else {
        anyhow::anyhow!("cannot prepare private hub state")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        fs::symlink_metadata(path).unwrap().mode() & 0o777
    }

    #[cfg(unix)]
    fn private_tempdir() -> tempfile::TempDir {
        let parent = std::env::temp_dir().canonicalize().unwrap();
        tempfile::tempdir_in(parent).unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn creates_an_absent_private_hierarchy_and_preserves_existing_state() {
        let home = private_tempdir();
        fs::set_permissions(home.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let hub = home.path().join(".cas/hub");

        ensure_private_dir(&hub).unwrap();

        assert_eq!(mode(&home.path().join(".cas")), 0o700);
        assert_eq!(mode(&hub), 0o700);
        fs::write(home.path().join(".cas/existing"), "keep").unwrap();
        ensure_private_dir(&hub).unwrap();
        assert_eq!(
            fs::read_to_string(home.path().join(".cas/existing")).unwrap(),
            "keep"
        );
    }

    #[cfg(unix)]
    #[test]
    fn preserves_a_safe_legacy_cas_parent_mode() {
        let home = private_tempdir();
        let cas = home.path().join(".cas");
        fs::create_dir(&cas).unwrap();
        fs::set_permissions(&cas, fs::Permissions::from_mode(0o775)).unwrap();
        fs::write(cas.join("existing"), "keep").unwrap();

        ensure_private_dir(&cas.join("hub")).unwrap();

        assert_eq!(mode(&cas), 0o775);
        assert_eq!(mode(&cas.join("hub")), 0o700);
        assert_eq!(fs::read_to_string(cas.join("existing")).unwrap(), "keep");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_file_loose_mode_and_unwritable_collisions_without_paths() {
        let cases = ["symlink", "file", "loose", "unwritable"];
        for case in cases {
            let home = private_tempdir();
            let cas = home.path().join(".cas");
            match case {
                "symlink" => {
                    let target = private_tempdir();
                    symlink(target.path(), &cas).unwrap();
                    let error = ensure_private_dir(&cas.join("hub"))
                        .unwrap_err()
                        .to_string();
                    assert!(error.contains("symlink or non-directory"));
                    assert!(!error.contains(home.path().to_string_lossy().as_ref()));
                }
                "file" => {
                    fs::write(&cas, "collision").unwrap();
                    let error = ensure_private_dir(&cas.join("hub"))
                        .unwrap_err()
                        .to_string();
                    assert!(error.contains("symlink or non-directory"));
                    assert!(!error.contains(home.path().to_string_lossy().as_ref()));
                }
                "loose" => {
                    fs::create_dir(&cas).unwrap();
                    let hub = cas.join("hub");
                    fs::create_dir(&hub).unwrap();
                    fs::set_permissions(&hub, fs::Permissions::from_mode(0o755)).unwrap();
                    let error = ensure_private_dir(&hub).unwrap_err().to_string();
                    assert!(error.contains("mode 0700"));
                    assert!(!error.contains(home.path().to_string_lossy().as_ref()));
                }
                "unwritable" => {
                    fs::create_dir(&cas).unwrap();
                    fs::set_permissions(&cas, fs::Permissions::from_mode(0o500)).unwrap();
                    let error = ensure_private_dir(&cas.join("hub"))
                        .unwrap_err()
                        .to_string();
                    assert!(error.contains("not owner-writable"));
                    assert!(!error.contains(home.path().to_string_lossy().as_ref()));
                    fs::set_permissions(&cas, fs::Permissions::from_mode(0o700)).unwrap();
                }
                _ => unreachable!(),
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_bootstrap_converges_on_one_private_hierarchy() {
        let home = private_tempdir();
        let hub = std::sync::Arc::new(home.path().join(".cas/hub"));
        let threads = (0..8)
            .map(|_| {
                let hub = hub.clone();
                std::thread::spawn(move || ensure_private_dir(&hub))
            })
            .collect::<Vec<_>>();

        for thread in threads {
            thread.join().unwrap().unwrap();
        }
        assert_eq!(mode(&home.path().join(".cas")), 0o700);
        assert_eq!(mode(&hub), 0o700);
    }
}
