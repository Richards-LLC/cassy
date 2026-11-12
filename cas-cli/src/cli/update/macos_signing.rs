//! A stable code identity for the installed binary on macOS (cas-f77b).
//!
//! Releases are ad-hoc signed, so their designated requirement is the cdhash
//! and every version is a new identity to TCC. Each privacy grant held by
//! `cas`, such as Desktop folder access or Developer Tools, stops matching
//! after an update, and macOS asks again. If the operator has created the
//! local identity described in docs/onboarding/macbook-from-zero.md, the update
//! re-signs the installed binary with it. The requirement then becomes this
//! identifier plus that certificate, which every later version keeps.

use std::path::Path;

/// Common name of the operator-created code-signing certificate.
pub(crate) const LOCAL_IDENTITY: &str = "Cassy Local Code Signing";
/// Signing identifier shared by every locally re-signed version.
pub(crate) const IDENTIFIER: &str = "dev.cassy.cas";

/// Re-sign `installed` with the local identity when the keychain holds one.
/// The signed copy replaces the binary by rename, so no reader sees a partly
/// written file. Returns whether the binary was re-signed.
#[cfg(target_os = "macos")]
pub(crate) fn stabilize(installed: &Path, keychain: Option<&Path>) -> anyhow::Result<bool> {
    use anyhow::Context;
    use std::process::Command;

    let mut find = Command::new("/usr/bin/security");
    find.args(["find-identity", "-p", "codesigning"]);
    if let Some(keychain) = keychain {
        find.arg(keychain);
    }
    let listing = find.output().context("could not list code-signing identities")?;
    // Rows read `  1) <40-hex SHA-1> "<name>"`, possibly followed by a trust note.
    let quoted = format!("\"{LOCAL_IDENTITY}\"");
    let Some(hash) = String::from_utf8_lossy(&listing.stdout).lines().find_map(|line| {
        let (_, rest) = line.split_once(") ")?;
        let (hash, name) = rest.split_once(' ')?;
        (hash.len() == 40 && name.starts_with(&quoted)).then(|| hash.to_owned())
    }) else {
        return Ok(false);
    };
    let name = installed.file_name().context("installed binary has no file name")?;
    let staged = installed.with_file_name(format!(
        ".{}.cas-sign-{}",
        name.to_string_lossy(),
        std::process::id()
    ));
    std::fs::copy(installed, &staged)
        .with_context(|| format!("could not stage {}", staged.display()))?;
    let mut sign = Command::new("/usr/bin/codesign");
    sign.args(["--force", "--sign", &hash, "--identifier", IDENTIFIER]);
    if let Some(keychain) = keychain {
        sign.arg("--keychain").arg(keychain);
    }
    let signed = sign.arg(&staged).output().context("could not run codesign")?;
    if !signed.status.success() {
        let _ = std::fs::remove_file(&staged);
        anyhow::bail!("codesign failed: {}", String::from_utf8_lossy(&signed.stderr).trim());
    }
    std::fs::rename(&staged, installed).with_context(|| {
        let _ = std::fs::remove_file(&staged);
        format!("could not replace {}", installed.display())
    })?;
    Ok(true)
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn stabilize(_installed: &Path, _keychain: Option<&Path>) -> anyhow::Result<bool> {
    Ok(false)
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::process::Command;

    fn run(program: &str, args: &[&str], dir: &Path) {
        let output = Command::new(program).args(args).current_dir(dir).output().unwrap();
        assert!(output.status.success(), "{program} {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    }

    fn requirement(binary: &Path) -> String {
        let output = Command::new("/usr/bin/codesign").args(["-d", "-r-"]).arg(binary).output().unwrap();
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    /// Two different releases keep one designated requirement, so a TCC grant
    /// made to the first still matches the second.
    #[test]
    fn local_identity_gives_every_update_one_designated_requirement_cas_f77b() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // A throwaway keychain; the user's keychain search list is not changed.
        std::fs::write(root.join("cert.cnf"), format!(
            "[req]\ndistinguished_name=dn\nx509_extensions=ext\nprompt=no\n[dn]\nCN={LOCAL_IDENTITY}\n\
             [ext]\nbasicConstraints=critical,CA:false\nkeyUsage=critical,digitalSignature\n\
             extendedKeyUsage=critical,codeSigning\n")).unwrap();
        run("/usr/bin/openssl", &["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", "key.pem",
            "-out", "cert.pem", "-days", "2", "-config", "cert.cnf"], root);
        run("/usr/bin/openssl", &["pkcs12", "-export", "-inkey", "key.pem", "-in", "cert.pem",
            "-out", "id.p12", "-passout", "pass:pw"], root);
        let keychain = root.join("test.keychain-db");
        let kc = keychain.to_str().unwrap();
        run("/usr/bin/security", &["create-keychain", "-p", "pw", kc], root);
        run("/usr/bin/security", &["unlock-keychain", "-p", "pw", kc], root);
        run("/usr/bin/security", &["import", "id.p12", "-k", kc, "-P", "pw", "-T", "/usr/bin/codesign"], root);
        run("/usr/bin/security", &["set-key-partition-list", "-S", "apple-tool:,apple:,codesign:", "-s", "-k", "pw", kc], root);

        // Two ad-hoc "releases" with different bytes, as the update installs them.
        let mut requirements = Vec::new();
        for (release, source) in [("old", "/usr/bin/true"), ("new", "/usr/bin/false")] {
            let installed = root.join(release).join("cas");
            std::fs::create_dir_all(installed.parent().unwrap()).unwrap();
            std::fs::copy(source, &installed).unwrap();
            run("/usr/bin/codesign", &["--force", "--sign", "-", installed.to_str().unwrap()], root);
            assert!(requirement(&installed).contains("cdhash"), "{}", requirement(&installed));
            assert!(stabilize(&installed, Some(&keychain)).unwrap());
            requirements.push(requirement(&installed));
        }
        assert_eq!(requirements[0], requirements[1]);
        assert!(requirements[0].contains(IDENTIFIER) && !requirements[0].contains("cdhash"), "{}", requirements[0]);
    }
}
