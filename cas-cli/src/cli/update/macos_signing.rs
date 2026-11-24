use anyhow::{Context, Result, ensure};
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::process::{Command, Output, Stdio};

const LOCAL_IDENTITY: &str = "Cassy Local Code Signing";
const IDENTIFIER: &str = "dev.cassy.cas";
const SECURITY: &str = "/usr/bin/security";
const OPENSSL: &str = "/usr/bin/openssl";
const CODESIGN: &str = "/usr/bin/codesign";

pub(super) fn stabilize(installed: &Path) -> Result<()> {
    let state = dirs::home_dir()
        .context("could not locate the home directory")?
        .join(".cas/macos-signing");
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&state)?;
    let metadata = state.symlink_metadata()?;
    ensure!(
        metadata.is_dir() && metadata.permissions().mode() & 0o077 == 0,
        "the local signing directory must be private"
    );
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(state.join("setup.lock"))?;
    fs2::FileExt::try_lock_exclusive(&lock)
        .context("could not lock the local signing directory")?;

    let keychain = state.join("cassy.keychain-db");
    let password_file = state.join("password");
    let pending = state.join("setup.pending");
    if setup_needed(&pending, &keychain, &password_file, delete_keychain)? {
        DirBuilder::new().mode(0o700).create(&pending)?;
        let setup = create_identity(&pending, &keychain, &password_file);
        setup_needed(&pending, &keychain, &password_file, delete_keychain)?;
        setup?;
    } else {
        let password =
            fs::read_to_string(&password_file).context("could not read signing password")?;
        let password = password.trim();
        ensure!(!password.is_empty(), "the signing password is empty");
        unlock_keychain(&keychain, password)?;
    }
    let listing = run(
        Command::new(SECURITY)
            .args(["find-identity", "-p", "codesigning"])
            .arg(&keychain),
        "find local code-signing identity",
    )?;
    let quoted = format!("\"{LOCAL_IDENTITY}\"");
    let listing = String::from_utf8_lossy(&listing.stdout);
    let mut identities = listing.lines().filter_map(|line| {
        let (_, rest) = line.split_once(") ")?;
        let (hash, name) = rest.split_once(' ')?;
        (hash.len() == 40
            && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            && name.starts_with(&quoted))
        .then_some(hash)
    });
    let hash = identities
        .next()
        .context("local code-signing identity not found")?;
    ensure!(
        identities.all(|identity| identity == hash),
        "multiple local code-signing identities found"
    );

    if verify(installed, hash).is_ok() {
        return Ok(());
    }
    let parent = installed
        .parent()
        .context("installed binary has no parent directory")?;
    let staged = tempfile::Builder::new()
        .prefix(".cas-sign-")
        .tempfile_in(parent)?;
    fs::copy(installed, staged.path()).context("could not stage a copy of the installed binary")?;
    run(
        &mut signing_command(&keychain, hash, staged.path()),
        "re-sign the staged binary",
    )?;
    verify(staged.path(), hash)?;
    staged
        .persist(installed)
        .context("could not replace the installed binary with the signed copy")?;
    Ok(())
}

fn setup_needed(
    pending: &Path,
    keychain: &Path,
    password_file: &Path,
    delete_keychain: impl FnOnce(&Path) -> Result<()>,
) -> Result<bool> {
    let mut has_keychain = regular_file_exists(keychain)?;
    let has_password = regular_file_exists(password_file)?;
    match pending.symlink_metadata() {
        Ok(metadata) => {
            ensure!(
                metadata.is_dir(),
                "the pending signing setup is not a directory"
            );
            if has_keychain && !has_password {
                delete_keychain(keychain)?;
                has_keychain = false;
            }
            fs::remove_dir_all(pending)?;
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    ensure!(
        has_keychain == has_password,
        "the local signing identity is incomplete"
    );
    Ok(!has_password)
}

fn regular_file_exists(path: &Path) -> Result<bool> {
    match path.symlink_metadata() {
        Ok(metadata) => {
            ensure!(
                metadata.is_file(),
                "{} is not a regular file",
                path.display()
            );
            Ok(true)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn delete_keychain(keychain: &Path) -> Result<()> {
    run(
        Command::new(SECURITY).arg("delete-keychain").arg(keychain),
        "remove incomplete signing keychain",
    )?;
    Ok(())
}

fn unlock_keychain(keychain: &Path, password: &str) -> Result<()> {
    run(
        Command::new(SECURITY)
            .args(["unlock-keychain", "-p", password])
            .arg(keychain),
        "unlock local signing keychain",
    )?;
    Ok(())
}

fn create_identity(materials: &Path, keychain: &Path, password_file: &Path) -> Result<()> {
    let password = run(
        Command::new(OPENSSL).args(["rand", "-hex", "32"]),
        "generate signing password",
    )?;
    let password = std::str::from_utf8(&password.stdout)?.trim();
    ensure!(
        !password.is_empty(),
        "could not generate the signing password"
    );
    let mut saved_password = tempfile::NamedTempFile::new_in(materials)?;
    saved_password.write_all(password.as_bytes())?;
    saved_password.as_file().sync_all()?;
    fs::write(
        materials.join("cert.cnf"),
        format!(
            "[req]\ndistinguished_name=dn\nx509_extensions=ext\nprompt=no\n[dn]\nCN={LOCAL_IDENTITY}\n\
                  [ext]\nbasicConstraints=critical,CA:false\nkeyUsage=critical,digitalSignature\n\
                  extendedKeyUsage=critical,codeSigning\n"
        ),
    )?;
    run(
        Command::new(OPENSSL).current_dir(materials).args([
            "req", "-batch", "-x509", "-newkey", "rsa:2048", "-sha256", "-nodes", "-keyout",
            "key.pem", "-out", "cert.pem", "-days", "3650", "-config", "cert.cnf",
        ]),
        "generate local code-signing certificate",
    )?;
    run(
        Command::new(OPENSSL)
            .current_dir(materials)
            .args([
                "pkcs12",
                "-export",
                "-inkey",
                "key.pem",
                "-in",
                "cert.pem",
                "-out",
                "identity.p12",
                "-passout",
                "env:CASSY_SIGNING_PASSWORD",
            ])
            .env("CASSY_SIGNING_PASSWORD", password),
        "package local code-signing identity",
    )?;

    run(
        Command::new(SECURITY)
            .args(["create-keychain", "-p", password])
            .arg(keychain),
        "create local signing keychain",
    )?;
    unlock_keychain(keychain, password)?;
    run(
        Command::new(SECURITY)
            .current_dir(materials)
            .args(["import", "identity.p12", "-k"])
            .arg(keychain)
            .args(["-P", password, "-T", CODESIGN]),
        "import local code-signing identity",
    )?;
    run(
        Command::new(SECURITY)
            .args([
                "set-key-partition-list",
                "-S",
                "apple-tool:,apple:,codesign:",
                "-s",
                "-k",
                password,
            ])
            .arg(keychain),
        "allow codesign to use the key non-interactively",
    )?;
    saved_password.persist_noclobber(password_file)?;
    Ok(())
}

fn requirement(hash: &str) -> String {
    format!("identifier \"{IDENTIFIER}\" and certificate leaf = H\"{hash}\"")
}

fn signing_command(keychain: &Path, hash: &str, staged: &Path) -> Command {
    let mut command = Command::new(CODESIGN);
    command
        .args([
            "--force",
            "--sign",
            hash,
            "--identifier",
            IDENTIFIER,
            "--timestamp=none",
            "--keychain",
        ])
        .arg(keychain)
        .arg("--requirements")
        .arg(format!("=designated => {}", requirement(hash)))
        .arg(staged);
    command
}

fn verify(binary: &Path, hash: &str) -> Result<()> {
    run(
        Command::new(CODESIGN)
            .arg("--verify")
            .arg("-R")
            .arg(format!("={}", requirement(hash)))
            .arg(binary),
        "verify the local code signature",
    )?;
    Ok(())
}

fn run(command: &mut Command, operation: &str) -> Result<Output> {
    let output = command
        .stdin(Stdio::null())
        .output()
        .with_context(|| operation.to_owned())?;
    if !output.status.success() && command.get_program() == CODESIGN {
        anyhow::bail!(
            "{operation}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    ensure!(
        output.status.success(),
        "{operation} failed ({})",
        output.status
    );
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_interrupted_setup_without_discarding_an_established_identity() {
        let state = tempfile::tempdir().unwrap();
        let pending = state.path().join("setup.pending");
        let keychain = state.path().join("cassy.keychain-db");
        let password_file = state.path().join("password");
        let remove_fixture = |path: &Path| Ok(fs::remove_file(path)?);

        fs::write(&keychain, b"keychain").unwrap();
        assert!(setup_needed(&pending, &keychain, &password_file, remove_fixture).is_err());
        assert!(keychain.exists());

        fs::create_dir(&pending).unwrap();
        fs::write(pending.join("identity.p12"), b"temporary identity").unwrap();
        assert!(setup_needed(&pending, &keychain, &password_file, remove_fixture).unwrap());
        assert!(!keychain.exists() && !pending.exists());

        fs::create_dir(&pending).unwrap();
        fs::write(&keychain, b"established keychain").unwrap();
        fs::write(&password_file, b"password").unwrap();
        assert!(!setup_needed(&pending, &keychain, &password_file, remove_fixture).unwrap());
        assert_eq!(fs::read(&keychain).unwrap(), b"established keychain");
        assert!(!pending.exists());
    }

    #[test]
    fn signs_with_the_dedicated_keychain_and_stable_requirement() {
        let keychain = Path::new("/private/cassy.keychain-db");
        let staged = Path::new("/private/install/.cas-sign-fixture");
        let hash = "0123456789ABCDEF0123456789ABCDEF01234567";
        let command = signing_command(keychain, hash, staged);
        let requirement_arg = format!(
            "=designated => identifier \"dev.cassy.cas\" and certificate leaf = H\"{hash}\""
        );
        assert_eq!(command.get_program(), CODESIGN);
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [
                "--force",
                "--sign",
                hash,
                "--identifier",
                "dev.cassy.cas",
                "--timestamp=none",
                "--keychain",
                keychain.to_str().unwrap(),
                "--requirements",
                &requirement_arg,
                staged.to_str().unwrap()
            ]
        );
    }
}
