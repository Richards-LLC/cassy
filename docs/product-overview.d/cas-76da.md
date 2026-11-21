## Core Concepts

A delivery merge grants publication authority over its source delivery, not
unrelated work already on the local target. Fresh origin evidence under the
target lock detects an ahead local target and refuses with its exact unpublished
commit list. That work requires a separate publication decision; the merge
preserves source, local target and origin. Ordinary and transactional publication
use the fence, while equal and behind targets keep their existing merge path.
