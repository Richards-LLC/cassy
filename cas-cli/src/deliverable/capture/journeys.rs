//! Goal-stage receipts, never a journey's entry page.
use super::*;

// Kept separate from catalog capture: the journey producer owns selection,
// assertions, stage framing and masks. Both evaluated revisions must pass.
pub(super) fn capture(
    _request: &CaptureRequest,
    _config: &Config,
    _capture: &QaCaptureConfig,
    _base: &str,
    _head: &str,
    _dir: &Path,
    _deadline: Deadline,
    started: Instant,
    mut receipt: CaptureReceipt,
) -> anyhow::Result<CaptureReceipt> {
    receipt.findings.push(pairs::finding(
        "capture-unavailable",
        "No shot catalog or passing goal-stage journey receipts; configure qa.capture.shots",
    ));
    receipt.elapsed_ms = started.elapsed().as_millis() as u64;
    Ok(receipt)
}
