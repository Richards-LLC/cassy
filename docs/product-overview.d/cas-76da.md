## Core Concepts

A delivery merge grants publication authority over its source delivery, not
unrelated work already on the local target. Fresh origin evidence under the
target lock detects unrelated unpublished target history and refuses with its
exact commit list. That work requires a separate publication decision; the merge
preserves source, local target and origin. Ordinary and transactional publication
use the fence, while equal and behind targets keep their existing merge path.
The delivery's own ancestry and its single integration merge are authorized.
Typed receipt failures persist before network reads, and detached candidate
validation preserves concurrent writers before a reviewed publication is refused.
