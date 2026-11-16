### Fixed — Factory

- **A parked delivery on a deleted train no longer stalls every train merge** — Was: one delivery still parked against a train whose branch was gone from origin made each daemon pass fail with "could not refresh train", so every active train merge stayed at 0 processed with no reason shown. Now: that train is skipped and logged once, other trains' conflict checks and merges advance in the same pass, and a train merge on the unavailable target itself shows the reason as its hold.
