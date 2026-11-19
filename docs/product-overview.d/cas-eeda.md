## Core Concepts

- Background task closes retain a local recovery journal until their terminal `CLOSE_OUTCOME` note is saved. Outcome recording participates in backend retirement. Lost continuations record an unknown mutation outcome and wake the owning supervisor; the daemon recovers unlocked journals after process exit without retrying the close mutation.
