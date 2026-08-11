//! Activity log widget for the director panel

use ratatui::layout::Rect;
use ratatui::Frame;
use std::collections::HashMap;

use crate::ui::factory::director::data::DirectorData;
use crate::ui::theme::ActiveTheme;
use crate::ui::widgets::render_compact_activity_list;

/// Render the activity section with optional focus indicator
pub fn render_with_focus(
    frame: &mut Frame,
    area: Rect,
    data: &DirectorData,
    theme: &ActiveTheme,
    focused: bool,
    _selected: Option<usize>,
    collapsed: bool,
    backend_tags: &HashMap<String, &'static str>,
) {
    if collapsed {
        super::panel::render_collapsed_header(
            frame,
            area,
            &theme.styles,
            super::panel::CollapsedHeader {
                title: "ACTIVITY",
                count: data.activity.len(),
                hotkey: None,
                focused,
                icon_style: None,
            },
        );
        return;
    }

    render_compact_activity_list(
        frame,
        area,
        &data.activity,
        &data.agent_id_to_name,
        theme,
        focused,
        backend_tags,
    );
}
