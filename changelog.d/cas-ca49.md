### Fixed — CI

- **Reuse the lane build after own tests** — Python helpers run by impact-selected tests no longer write bytecode into the watched source tree, avoiding an unnecessary cas rebuild when inventorying the ring. Test selection and attribution remain unchanged.
