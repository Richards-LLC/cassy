### Fixed — CI

- **Reuse the lane build after own tests** — Python helpers run by impact-selected tests no longer write bytecode into the watched source tree, avoiding an unnecessary cas rebuild when inventorying the ring. The measured post-test build fell from 302 s to 0.59 s with the bytecode producer selected, preserving all 11,533 executed native test identities; the code tip after probe removal recorded 0.25 s. Test selection and attribution remain unchanged.
