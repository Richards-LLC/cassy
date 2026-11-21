### Fixed — Verification

- **A provider safety refusal no longer kills the automatic review** — Was:
  when the Codex provider's safety filter refused to review a delivery (for
  example a credential-read guard), the verifier exited 1 and the dispatch
  was released with no review and no classification. Now: the refusal is
  recorded on that exact dispatch as an infrastructure ERROR, never a code
  verdict, with the refused output kept on disk; the same dispatch is then
  reviewed once on the Claude lane. When no other lane can start, the
  supervisor gets one notice with the direct-verdict route. Ordinary code
  rejections and authentication failures keep their existing handling.
