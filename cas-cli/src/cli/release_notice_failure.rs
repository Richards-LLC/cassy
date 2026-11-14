//! Terminal installer notice failures use the release's owner and durable outbox.
use std::{num::NonZeroU32, path::Path};

use anyhow::Result;

pub(super) fn record(
    root: &Path,
    repo: &str,
    version: &str,
    attempts: NonZeroU32,
    receipt: &Path,
) -> Result<bool> {
    let Some(context) = super::release_automation::tracked_context(root, repo, version)? else {
        return Ok(false);
    };
    let mut train = crate::store::open_task_store(root)?.get(&context.train_id)?;
    train.epic_verification_owner = Some(context.owner);
    crate::supervisor_notices::release_action(
        root,
        &train,
        crate::supervisor_notices::ReleaseNoticePhase::PostPublishFailed,
        version,
        &format!(
            "v{version} post-release notice failed after {attempts} attempts; automatic attempts stopped. \
             Failed receipt: {}. Repair the cause, then retry with \
             cas release installed --receipt '{}' --repo {repo}.",
            receipt.display(), receipt.display(),
        ),
    )?;
    Ok(true)
}
