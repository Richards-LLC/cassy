### Fixed — CI

- **Attribute merged train failures to the contained target** — Lane attribution now uses the merge-base with the live target, so failures brought in by a train merge are identified as inherited. A valid recorded start base remains a fallback when the target cannot be resolved; delivery-caused failures stay red.
