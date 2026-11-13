## Core Concepts

**Merge response receipts.** A slow delivery merge returns a preparing receipt after a short wait while its existing safety checks continue in the background. The receipt path records the complete final result, and a supervisor notice reports completion. A published merge reports COMMITTED even when its close follow-up is still running. Callers use the receipt and notice before deciding whether another merge request is needed.
