//! Pre-composed styles for common UI patterns

use ratatui::style::{Modifier, Style};

use crate::ui::theme::palette::Palette;

/// Pre-composed styles for consistent UI rendering
#[derive(Debug, Clone)]
pub struct Styles {
    // Text hierarchy
    pub text_primary: Style,
    pub text_secondary: Style,
    pub text_muted: Style,
    pub text_disabled: Style,
    pub text_accent: Style,
    pub text_bold: Style,
    pub text_label: Style,

    // Status text
    pub text_success: Style,
    pub text_warning: Style,
    pub text_error: Style,
    pub text_info: Style,

    // Backgrounds
    pub bg_primary: Style,
    pub bg_secondary: Style,
    pub bg_elevated: Style,
    pub bg_selection: Style,

    // Borders
    pub border_default: Style,
    pub border_focused: Style,
    pub border_muted: Style,

    // List items
    pub list_item: Style,
    pub list_item_selected: Style,

    // Entity IDs
    pub entity_id: Style,
    pub entity_type: Style,

    // Headers
    pub header_title: Style,
    pub header_subtitle: Style,
    pub section_title: Style,

    // Tabs
    pub tab_inactive: Style,
    pub tab_active: Style,

    // Status bar
    pub status_bar: Style,
    pub status_bar_key: Style,

    // Input
    pub input_normal: Style,
    pub input_focused: Style,
    pub input_placeholder: Style,

    // Scores/feedback
    pub score_positive: Style,
    pub score_negative: Style,
    pub score_neutral: Style,

    // Task status
    pub task_open: Style,
    pub task_in_progress: Style,
    pub task_blocked: Style,
    pub task_closed: Style,

    // Priority
    pub priority_critical: Style,
    pub priority_high: Style,
    pub priority_medium: Style,
    pub priority_low: Style,
    pub priority_backlog: Style,

    // Agent status
    pub agent_active: Style,
    pub agent_idle: Style,
    pub agent_dead: Style,

    // Skill status
    pub skill_enabled: Style,
    pub skill_disabled: Style,
    pub skill_draft: Style,

    // Rule status
    pub rule_draft: Style,
    pub rule_proven: Style,
    pub rule_stale: Style,
    pub rule_retired: Style,

    // Verification
    pub verification_approved: Style,
    pub verification_rejected: Style,
    pub verification_error: Style,
    pub verification_skipped: Style,

    // Worktree
    pub worktree_active: Style,
    pub worktree_merged: Style,
    pub worktree_abandoned: Style,
    pub worktree_conflict: Style,
    pub worktree_removed: Style,
    pub worktree_orphaned: Style,

    // Dashboard metrics
    pub metric_value: Style,
    pub trend_positive: Style,
    pub trend_negative: Style,

    // Bar charts
    pub bar_default: Style,
    pub bar_success: Style,
    pub bar_warning: Style,
    pub bar_error: Style,
    pub bar_info: Style,

    // Heatmap
    pub heatmap_empty: Style,
    pub heatmap_low: Style,
    pub heatmap_medium: Style,
    pub heatmap_high: Style,
    pub heatmap_max: Style,

    // Contextual hints
    pub hint_key: Style,
    pub hint_key_primary: Style,
    pub hint_description: Style,

    // Keybind hint-key colors (cas-7bf4) — status-bar right-side hint row
    // (^P focus, ^Q quit, etc). Same semantic mapping as text_info/success/
    // warning/error/accent but split so light mode can be darkened for
    // legibility without touching the shared status/accent chip-bg colors.
    pub hint_info: Style,
    pub hint_success: Style,
    pub hint_warning: Style,
    pub hint_error: Style,
    pub hint_accent: Style,

    // Markdown rendering
    pub md_h1: Style,
    pub md_h2: Style,
    pub md_h3: Style,
    pub md_code_inline: Style,
    pub md_code_block: Style,
    pub md_blockquote: Style,
    pub md_link: Style,
}

impl Styles {
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            // Text
            text_primary: Style::default().fg(p.text_primary),
            text_secondary: Style::default().fg(p.text_secondary),
            text_muted: Style::default().fg(p.text_muted),
            text_disabled: Style::default().fg(p.text_disabled),
            text_accent: Style::default().fg(p.text_accent),
            text_bold: Style::default()
                .fg(p.text_primary)
                .add_modifier(Modifier::BOLD),
            text_label: Style::default()
                .fg(p.text_secondary)
                .add_modifier(Modifier::BOLD),

            // Status text
            text_success: Style::default().fg(p.text_success),
            text_warning: Style::default().fg(p.text_warning),
            text_error: Style::default().fg(p.text_error),
            text_info: Style::default().fg(p.text_info),

            // Backgrounds
            bg_primary: Style::default().bg(p.bg_primary),
            bg_secondary: Style::default().bg(p.bg_secondary),
            bg_elevated: Style::default().bg(p.bg_elevated),
            bg_selection: Style::default()
                .bg(p.selection_bg)
                .add_modifier(Modifier::BOLD),

            // Borders
            border_default: Style::default().fg(p.border_default),
            border_focused: Style::default().fg(p.border_focused),
            border_muted: Style::default().fg(p.border_muted),

            // Lists
            list_item: Style::default().fg(p.text_primary),
            list_item_selected: Style::default()
                .bg(p.selection_bg)
                .fg(p.text_primary)
                .add_modifier(Modifier::BOLD),

            // Entities
            entity_id: Style::default().fg(p.entity_id),
            entity_type: Style::default().fg(p.entity_type),

            // Headers
            header_title: Style::default()
                .fg(p.text_primary)
                .add_modifier(Modifier::BOLD),
            header_subtitle: Style::default().fg(p.text_secondary),
            section_title: Style::default()
                .fg(p.text_accent)
                .add_modifier(Modifier::BOLD),

            // Tabs
            tab_inactive: Style::default().fg(p.text_muted),
            tab_active: Style::default()
                .fg(p.text_accent)
                .add_modifier(Modifier::BOLD),

            // Status bar
            status_bar: Style::default().fg(p.text_muted),
            status_bar_key: Style::default()
                .fg(p.text_secondary)
                .add_modifier(Modifier::BOLD),

            // Input
            input_normal: Style::default().fg(p.text_primary),
            input_focused: Style::default().fg(p.text_accent),
            input_placeholder: Style::default().fg(p.text_muted),

            // Scores
            score_positive: Style::default().fg(p.score_positive),
            score_negative: Style::default().fg(p.score_negative),
            score_neutral: Style::default().fg(p.score_neutral),

            // Task status
            task_open: Style::default().fg(p.task_open),
            task_in_progress: Style::default().fg(p.task_in_progress),
            task_blocked: Style::default().fg(p.task_blocked),
            task_closed: Style::default().fg(p.task_closed),

            // Priority
            priority_critical: Style::default()
                .fg(p.priority_critical)
                .add_modifier(Modifier::BOLD),
            priority_high: Style::default().fg(p.priority_high),
            priority_medium: Style::default().fg(p.priority_medium),
            priority_low: Style::default().fg(p.priority_low),
            priority_backlog: Style::default().fg(p.priority_backlog),

            // Agent status
            agent_active: Style::default().fg(p.agent_active),
            agent_idle: Style::default().fg(p.agent_idle),
            agent_dead: Style::default().fg(p.agent_dead),

            // Skill status
            skill_enabled: Style::default().fg(p.skill_enabled),
            skill_disabled: Style::default().fg(p.skill_disabled),
            skill_draft: Style::default().fg(p.skill_draft),

            // Rule status
            rule_draft: Style::default().fg(p.rule_draft),
            rule_proven: Style::default().fg(p.rule_proven),
            rule_stale: Style::default().fg(p.rule_stale),
            rule_retired: Style::default().fg(p.rule_retired),

            // Verification
            verification_approved: Style::default().fg(p.verification_approved),
            verification_rejected: Style::default().fg(p.verification_rejected),
            verification_error: Style::default().fg(p.verification_error),
            verification_skipped: Style::default().fg(p.verification_skipped),

            // Worktree
            worktree_active: Style::default().fg(p.worktree_active),
            worktree_merged: Style::default().fg(p.worktree_merged),
            worktree_abandoned: Style::default().fg(p.worktree_abandoned),
            worktree_conflict: Style::default().fg(p.worktree_conflict),
            worktree_removed: Style::default().fg(p.worktree_removed),
            worktree_orphaned: Style::default().fg(p.worktree_orphaned),

            // Dashboard metrics
            metric_value: Style::default()
                .fg(p.metric_value)
                .add_modifier(Modifier::BOLD),
            trend_positive: Style::default().fg(p.trend_positive),
            trend_negative: Style::default().fg(p.trend_negative),

            // Bar charts
            bar_default: Style::default().fg(p.bar_default),
            bar_success: Style::default().fg(p.bar_success),
            bar_warning: Style::default().fg(p.bar_warning),
            bar_error: Style::default().fg(p.bar_error),
            bar_info: Style::default().fg(p.bar_info),

            // Heatmap
            heatmap_empty: Style::default().fg(p.heatmap_empty),
            heatmap_low: Style::default().fg(p.heatmap_low),
            heatmap_medium: Style::default().fg(p.heatmap_medium),
            heatmap_high: Style::default().fg(p.heatmap_high),
            heatmap_max: Style::default().fg(p.heatmap_max),

            // Contextual hints
            hint_key: Style::default().fg(p.hint_key),
            hint_key_primary: Style::default()
                .fg(p.hint_key_primary)
                .add_modifier(Modifier::BOLD),
            hint_description: Style::default().fg(p.hint_description),

            // Keybind hint-key colors (cas-7bf4)
            hint_info: Style::default().fg(p.hint_info),
            hint_success: Style::default().fg(p.hint_success),
            hint_warning: Style::default().fg(p.hint_warning),
            hint_error: Style::default().fg(p.hint_error),
            hint_accent: Style::default().fg(p.hint_accent),

            // Markdown rendering
            md_h1: Style::default()
                .fg(p.accent)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            md_h2: Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
            md_h3: Style::default()
                .fg(p.text_primary)
                .add_modifier(Modifier::BOLD),
            md_code_inline: Style::default().fg(p.colors.cyan).bg(p.bg_secondary),
            md_code_block: Style::default().fg(p.text_secondary),
            md_blockquote: Style::default()
                .fg(p.text_muted)
                .add_modifier(Modifier::ITALIC),
            md_link: Style::default()
                .fg(p.status_info)
                .add_modifier(Modifier::UNDERLINED),
        }
    }
}
