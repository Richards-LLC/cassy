//! Shared UI widgets for sidecar and factory TUI
//!
//! Reusable stateless widgets for rendering CAS data.
//! These widgets can be used by both the standalone sidecar
//! and the integrated factory TUI panels.

mod activity;
mod agents;
mod changes;
mod tasks;
mod utils;

pub use activity::{
    format_activity_summary, render_activity_list, render_activity_list_with_state,
    render_compact_activity_list, ActivityConfig,
};
pub use agents::{
    render_agent_list, render_agent_list_with_state, render_compact_agent_list, AgentConfig,
    AgentDisplayInfo,
};
pub use changes::{
    build_changes_items_with_types, calculate_changes_height, render_changes_list,
    render_changes_list_with_collapsed, render_changes_list_with_state,
    render_compact_changes_list, render_compact_changes_list_with_state, ChangesConfig,
    FileChangeInfo, GitFileStatus, SourceChangesInfo, TreeItemType,
};
pub use tasks::{
    build_task_item, render_compact_task_list, render_task_list, render_task_list_with_state,
    TaskConfig, TaskDisplayInfo,
};
pub use utils::{event_type_color, format_relative, priority_color, truncate, truncate_to_width};
