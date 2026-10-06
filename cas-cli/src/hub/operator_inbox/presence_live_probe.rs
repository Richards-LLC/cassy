//! Explicit disposable-account production probe; never part of normal suites.
//! A separate browser drives consent/notices and writes the control signals.

use super::super::*;
use crate::hub::operator_inbox::machine::{
    AccountAuthority, EnrollRequest, PrincipalKind, enroll_machine, issuer_keys, revoke_principal,
};

const CLOUD: &str = "https://petra-stella-cloud.vercel.app";

fn required(name: &str) -> anyhow::Result<String> {
    std::env::var(name).map_err(|_| anyhow::anyhow!("missing {name}"))
}

async fn signal(root: &Path, name: &str, deadline: tokio::time::Instant) -> anyhow::Result<()> {
    while !root.join(name).is_file() {
        anyhow::ensure!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {name}"
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Ok(())
}

async fn lifecycle(root: &Path) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(900);
    signal(root, "start", deadline).await?;
    let first = spawn_presence_loop(root.to_owned());
    let stopped = signal(root, "stop", deadline).await;
    first.abort();
    let _ = first.await;
    // Let any bounded blocking HTTP exchange finish before declaring silence.
    tokio::time::sleep(Duration::from_secs(11)).await;
    stopped?;
    fs::write(
        root.join("producer-stopped"),
        chrono::Utc::now().to_rfc3339(),
    )?;
    signal(root, "restart", deadline).await?;
    let second = spawn_presence_loop(root.to_owned());
    let finished = signal(root, "finish", deadline).await;
    second.abort();
    let _ = second.await;
    tokio::time::sleep(Duration::from_secs(11)).await;
    finished
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "production mutation: requires approved disposable account and separate live browser"]
async fn presence_live_disposable_producer() -> anyhow::Result<()> {
    anyhow::ensure!(required("CAS_E3DD_LIVE")? == "1", "CAS_E3DD_LIVE must be 1");
    let account_id = required("CAS_E3DD_ACCOUNT_ID")?;
    anyhow::ensure!(
        uuid::Uuid::parse_str(&account_id).is_ok(),
        "invalid test account id"
    );
    let key_path = PathBuf::from(required("CAS_E3DD_API_KEY_FILE")?);
    let key_meta = fs::symlink_metadata(&key_path)?;
    anyhow::ensure!(key_meta.is_file(), "API key must be a regular file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        anyhow::ensure!(
            key_meta.permissions().mode() & 0o777 == 0o600,
            "API key file must be 0600"
        );
    }
    let bearer = cas_operator_crypto::Zeroizing::new(fs::read_to_string(key_path)?);
    let http: Arc<dyn HttpClient> = Arc::new(UreqHttp::default());
    // Confirm the approved account before the first mutation. No CLI config,
    // HOME lookup, or implicit enrollment authority is used by this probe.
    let me = http.send(
        "GET",
        &format!("{CLOUD}/api/me"),
        &[("Authorization", format!("Bearer {}", bearer.trim()))],
        &[],
    )?;
    anyhow::ensure!(
        me.status == 200,
        "test account identity refused ({})",
        me.status
    );
    let identity: Value = serde_json::from_slice(&me.body)?;
    anyhow::ensure!(
        identity["user_id"].as_str() == Some(account_id.as_str()),
        "test account id mismatch"
    );
    anyhow::ensure!(
        identity["email"]
            .as_str()
            .is_some_and(|email| email.split('@').next() == Some("cassy-e3dd-test")),
        "account must be labeled cassy-e3dd-test"
    );
    let root = PathBuf::from(required("CAS_E3DD_LIVE_ROOT")?);
    anyhow::ensure!(
        root.is_absolute()
            && root
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("cas-e3dd-live-")),
        "root must be a fresh absolute cas-e3dd-live-* directory"
    );
    fs::create_dir(&root)?; // refuses an existing enrollment or pre-existing root
    crate::hub::ensure_private_dir(&root)?;
    let authority = AccountAuthority::new(CLOUD, bearer.trim().to_owned());
    let issuer = issuer_keys(Arc::clone(&http), CLOUD);
    let label = format!("cassy-e3dd-test-{}", uuid::Uuid::new_v4());
    let principal = enroll_machine(
        http.as_ref(),
        &authority,
        &issuer,
        &EnrollRequest {
            hub_id: &label,
            label: &label,
            projects: &["cas-e3dd-live".to_owned()],
            presence: true,
        },
    )?;
    let result = async {
        anyhow::ensure!(
            principal.account_id == account_id,
            "enrollment account mismatch"
        );
        PrincipalStore::new(&root).save(&principal)?;
        fs::write(
            root.join("ready.json"),
            serde_json::to_vec(&json!({
                "account_id": account_id, "machine_id": principal.machine_id, "label": label,
                "control": ["start", "stop", "restart", "finish"],
            }))?,
        )?;
        lifecycle(&root).await
    }
    .await;
    // Every ordinary error/timeout goes through cleanup. The account remains
    // available for cas-2077; revoke only the machine created by this probe.
    let cleanup = revoke_principal(
        http.as_ref(),
        &authority,
        PrincipalKind::Machine,
        &principal.machine_id,
    );
    if cleanup.is_ok() {
        fs::write(
            root.join("machine-revoked.json"),
            serde_json::to_vec(&json!({"machine_id": principal.machine_id}))?,
        )?;
        let _ = fs::remove_file(root.join("operator-inbox").join("machine.json"));
    }
    cleanup?;
    result
}
