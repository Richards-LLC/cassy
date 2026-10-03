//! Operator-facing operations shared by MCP and the Commander hub
//! (fleet-operations brief, cas-9886). Each operation is one function both
//! surfaces call, so they share preconditions, effects and audit semantics.

pub(crate) mod fleet;
