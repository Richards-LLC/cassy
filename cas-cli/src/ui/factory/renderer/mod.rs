//! Factory view mode and focus types.

use super::director::SidecarFocus;

/// Which top-level view the factory TUI is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FactoryViewMode {
    /// Standard pane layout (workers + supervisor + sidecar).
    #[default]
    Panes,
    /// Mission Control dashboard (no PTY panes, all panels).
    MissionControl,
}

/// Which panel has focus in Mission Control view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MissionControlFocus {
    #[default]
    None,
    Workers,
    Tasks,
    Changes,
    Activity,
}

impl MissionControlFocus {
    /// Cycle to the next focusable panel.
    pub fn next(self) -> Self {
        match self {
            Self::None => Self::Workers,
            Self::Workers => Self::Tasks,
            Self::Tasks => Self::Changes,
            Self::Changes => Self::Activity,
            Self::Activity => Self::Workers,
        }
    }

    /// Cycle to the previous focusable panel.
    pub fn prev(self) -> Self {
        match self {
            Self::None => Self::Activity,
            Self::Workers => Self::Activity,
            Self::Tasks => Self::Workers,
            Self::Changes => Self::Tasks,
            Self::Activity => Self::Changes,
        }
    }

    /// Convert to a SidecarFocus for shared PanelRegistry access.
    pub fn to_sidecar_focus(self) -> SidecarFocus {
        match self {
            Self::None => SidecarFocus::None,
            Self::Workers => SidecarFocus::Factory,
            Self::Tasks => SidecarFocus::Tasks,
            Self::Changes => SidecarFocus::Changes,
            Self::Activity => SidecarFocus::Activity,
        }
    }

    /// Panel display name for the status bar.
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Workers => "Workers",
            Self::Tasks => "Tasks",
            Self::Changes => "Changes",
            Self::Activity => "Activity",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_factory_view_mode_default_is_panes() {
        let mode = FactoryViewMode::default();
        assert_eq!(mode, FactoryViewMode::Panes);
    }

    #[test]
    fn test_factory_view_mode_toggle() {
        let mut app = crate::ui::factory::app::FactoryApp::for_test();
        assert_eq!(app.factory_view_mode, FactoryViewMode::Panes);
        app.toggle_factory_view_mode();
        assert_eq!(app.factory_view_mode, FactoryViewMode::MissionControl);
        app.toggle_factory_view_mode();
        assert_eq!(app.factory_view_mode, FactoryViewMode::Panes);
    }
}
