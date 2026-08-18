//! Semantic color palette - what colors mean in context

use ratatui::style::Color;

use crate::ui::theme::colors::ColorPalette;

/// Semantic color palette mapping colors to UI meanings
#[derive(Debug, Clone)]
pub struct Palette {
    pub colors: ColorPalette,

    // Core UI
    pub bg_primary: Color,
    pub bg_secondary: Color,
    pub bg_elevated: Color,
    pub bg_overlay: Color,

    pub text_primary: Color,
    pub text_secondary: Color,
    pub text_muted: Color,
    pub text_disabled: Color,

    pub border_default: Color,
    pub border_focused: Color,
    pub border_muted: Color,

    // Interactive
    pub accent: Color,
    pub accent_dim: Color,
    pub selection_bg: Color,
    pub highlight_bg: Color,

    // Status indicators
    pub status_success: Color,
    pub status_warning: Color,
    pub status_error: Color,
    pub status_info: Color,
    pub status_neutral: Color,

    // Task status
    pub task_open: Color,
    pub task_in_progress: Color,
    pub task_blocked: Color,
    pub task_closed: Color,

    // Priority levels
    pub priority_critical: Color,
    pub priority_high: Color,
    pub priority_medium: Color,
    pub priority_low: Color,
    pub priority_backlog: Color,

    // Entity types
    pub entity_id: Color,
    pub entity_type: Color,
    pub entity_skill: Color,
    pub entity_rule: Color,
    pub entity_agent: Color,

    // Agent status
    pub agent_active: Color,
    pub agent_idle: Color,
    pub agent_dead: Color,

    // Skill status
    pub skill_enabled: Color,
    pub skill_disabled: Color,
    pub skill_draft: Color,

    // Rule status
    pub rule_draft: Color,
    pub rule_proven: Color,
    pub rule_stale: Color,
    pub rule_retired: Color,

    // Verification
    pub verification_approved: Color,
    pub verification_rejected: Color,
    pub verification_error: Color,
    pub verification_skipped: Color,

    // Worktree status
    pub worktree_active: Color,
    pub worktree_merged: Color,
    pub worktree_abandoned: Color,
    pub worktree_conflict: Color,
    pub worktree_removed: Color,
    pub worktree_orphaned: Color,

    // Feedback scores
    pub score_positive: Color,
    pub score_negative: Color,
    pub score_neutral: Color,

    // Dashboard metrics
    pub metric_value: Color,
    pub trend_positive: Color,
    pub trend_negative: Color,

    // Bar charts
    pub bar_default: Color,
    pub bar_success: Color,
    pub bar_warning: Color,
    pub bar_error: Color,
    pub bar_info: Color,

    // Heatmap
    pub heatmap_empty: Color,
    pub heatmap_low: Color,
    pub heatmap_medium: Color,
    pub heatmap_high: Color,
    pub heatmap_max: Color,

    // Contextual hints
    pub hint_key: Color,
    pub hint_key_primary: Color,
    pub hint_description: Color,

    // Chip/badge foreground — fixed near-black, used for short badge labels
    // (mode indicators, banners) rendered on saturated chip backgrounds
    // (status_info/warning/success/accent). Those backgrounds are
    // medium-bright in BOTH theme modes, so a fixed dark foreground beats
    // reusing text_primary (which flips to near-white in dark mode and
    // fails contrast against a medium-bright chip fill — see cas-7bf4).
    pub chip_fg: Color,

    // Keybind hint-key foreground — status_info/success/warning/error/accent
    // rendered as short (often bold) key labels directly on bg_elevated
    // (the status bar's right-side hint row), NOT as a chip background.
    // In dark mode these equal the status/accent colors unchanged (already
    // ≥4.5:1 there). In light mode the status/accent colors themselves are
    // too pale against bg_elevated (as low as 1.54:1) — these are darker
    // variants of the SAME hue reserved for this foreground-on-bg_elevated
    // role, split out precisely so status_info/success/warning/error/accent
    // can stay bright for their OTHER role as chip backgrounds (cas-7bf4).
    pub hint_info: Color,
    pub hint_success: Color,
    pub hint_warning: Color,
    pub hint_error: Color,
    pub hint_accent: Color,
}

impl Palette {
    pub fn from_colors(colors: ColorPalette, is_dark: bool) -> Self {
        if is_dark {
            Self::dark_from_colors(colors)
        } else {
            Self::light_from_colors(colors)
        }
    }

    fn dark_from_colors(colors: ColorPalette) -> Self {
        Self {
            // Core UI
            bg_primary: colors.gray_900,
            bg_secondary: colors.gray_800,
            bg_elevated: colors.gray_700,
            bg_overlay: Color::Rgb(0, 0, 0),

            text_primary: colors.gray_100,
            text_secondary: colors.gray_300,
            text_muted: colors.gray_400,
            text_disabled: colors.gray_500,

            border_default: colors.gray_400,
            border_focused: colors.primary_400,
            border_muted: colors.gray_400,

            // Interactive
            accent: colors.primary_400,
            accent_dim: colors.primary_500,
            selection_bg: colors.gray_700,
            highlight_bg: colors.primary_500,

            // Status
            status_success: colors.success,
            status_warning: colors.warning,
            status_error: colors.error,
            status_info: colors.info,
            status_neutral: colors.gray_400,

            // Task status
            task_open: colors.gray_400,
            task_in_progress: colors.info,
            task_blocked: colors.warning,
            task_closed: colors.success,

            // Priority
            priority_critical: colors.error,
            priority_high: Color::Rgb(255, 120, 100),
            priority_medium: colors.warning,
            priority_low: colors.gray_400,
            priority_backlog: colors.gray_500,

            // Entities
            entity_id: colors.cyan,
            entity_type: colors.warning,
            entity_skill: colors.purple,
            entity_rule: colors.success,
            entity_agent: colors.info,

            // Agent status
            agent_active: colors.success,
            agent_idle: colors.warning,
            agent_dead: colors.error,

            // Skill status
            skill_enabled: colors.success,
            skill_disabled: colors.gray_500,
            skill_draft: colors.warning,

            // Rule status
            rule_draft: colors.gray_400,
            rule_proven: colors.success,
            rule_stale: colors.warning,
            rule_retired: colors.error,

            // Verification
            verification_approved: colors.success,
            verification_rejected: colors.error,
            verification_error: colors.purple,
            verification_skipped: colors.gray_500,

            // Worktree
            worktree_active: colors.success,
            worktree_merged: colors.cyan,
            worktree_abandoned: colors.error,
            worktree_conflict: colors.purple,
            worktree_removed: colors.gray_500,
            worktree_orphaned: colors.warning,

            // Feedback
            score_positive: colors.success,
            score_negative: colors.error,
            score_neutral: colors.gray_500,

            // Dashboard metrics
            metric_value: colors.gray_100,
            trend_positive: colors.success,
            trend_negative: colors.error,

            // Bar charts
            bar_default: colors.primary_400,
            bar_success: colors.success,
            bar_warning: colors.warning,
            bar_error: colors.error,
            bar_info: colors.info,

            // Heatmap
            heatmap_empty: colors.gray_800,
            heatmap_low: Color::Rgb(70, 140, 70),
            heatmap_medium: Color::Rgb(80, 160, 80),
            heatmap_high: Color::Rgb(100, 200, 100),
            heatmap_max: colors.success,

            // Contextual hints
            hint_key: colors.gray_400,
            hint_key_primary: colors.primary_400,
            hint_description: colors.gray_400,

            // Chip/badge foreground (cas-7bf4 contrast fix)
            chip_fg: Color::Rgb(15, 16, 20),

            // Keybind hint-key foreground (cas-7bf4 contrast fix) — dark
            // mode's status/accent colors already clear 4.5:1 against
            // bg_elevated, so these are unchanged aliases.
            hint_info: colors.info,
            hint_success: colors.success,
            hint_warning: colors.warning,
            hint_error: colors.error,
            hint_accent: colors.primary_400,

            colors,
        }
    }

    // MAINTENANCE: light_from_colors mirrors dark_from_colors field-for-field.
    // The light palette works via ColorPalette inversion (gray_50↔gray_900 etc.).
    // When adding a new semantic color to dark_from_colors, you MUST also add it here.
    fn light_from_colors(colors: ColorPalette) -> Self {
        Self {
            // Core UI - inverted for light
            bg_primary: colors.gray_900,
            bg_secondary: colors.gray_800,
            bg_elevated: colors.gray_700,
            bg_overlay: Color::Rgb(255, 255, 255),

            text_primary: colors.gray_100,
            text_secondary: colors.gray_300,
            text_muted: colors.gray_500,
            text_disabled: colors.gray_600,

            border_default: colors.gray_600,
            border_focused: colors.primary_400,
            border_muted: colors.gray_600,

            // Interactive
            accent: colors.primary_400,
            accent_dim: colors.primary_500,
            selection_bg: colors.gray_700,
            highlight_bg: colors.primary_500,

            // Status
            status_success: colors.success,
            status_warning: colors.warning,
            status_error: colors.error,
            status_info: colors.info,
            status_neutral: colors.gray_400,

            // Task status
            task_open: colors.gray_400,
            task_in_progress: colors.info,
            task_blocked: colors.warning,
            task_closed: colors.success,

            // Priority
            priority_critical: colors.error,
            priority_high: Color::Rgb(200, 80, 60),
            priority_medium: colors.warning,
            priority_low: colors.gray_500,
            priority_backlog: colors.gray_500,

            // Entities
            entity_id: colors.cyan,
            entity_type: colors.warning,
            entity_skill: colors.purple,
            entity_rule: colors.success,
            entity_agent: colors.info,

            // Agent status
            agent_active: colors.success,
            agent_idle: colors.warning,
            agent_dead: colors.error,

            // Skill status
            skill_enabled: colors.success,
            skill_disabled: colors.gray_500,
            skill_draft: colors.warning,

            // Rule status
            rule_draft: colors.gray_400,
            rule_proven: colors.success,
            rule_stale: colors.warning,
            rule_retired: colors.error,

            // Verification
            verification_approved: colors.success,
            verification_rejected: colors.error,
            verification_error: colors.purple,
            verification_skipped: colors.gray_500,

            // Worktree
            worktree_active: colors.success,
            worktree_merged: colors.cyan,
            worktree_abandoned: colors.error,
            worktree_conflict: colors.purple,
            worktree_removed: colors.gray_500,
            worktree_orphaned: colors.warning,

            // Feedback
            score_positive: colors.success,
            score_negative: colors.error,
            score_neutral: colors.gray_500,

            // Dashboard metrics
            metric_value: colors.gray_900,
            trend_positive: colors.success,
            trend_negative: colors.error,

            // Bar charts
            bar_default: colors.primary_400,
            bar_success: colors.success,
            bar_warning: colors.warning,
            bar_error: colors.error,
            bar_info: colors.info,

            // Heatmap
            heatmap_empty: colors.gray_300,
            heatmap_low: Color::Rgb(150, 220, 150),
            heatmap_medium: Color::Rgb(100, 180, 100),
            heatmap_high: Color::Rgb(60, 140, 60),
            heatmap_max: colors.success,

            // Contextual hints
            hint_key: colors.gray_600,
            hint_key_primary: colors.primary_400,
            hint_description: colors.gray_500,

            // Chip/badge foreground (cas-7bf4 contrast fix) — same fixed
            // near-black as dark; chip backgrounds are medium-bright in
            // both modes so one dark foreground covers both.
            chip_fg: Color::Rgb(15, 16, 20),

            // Keybind hint-key foreground (cas-7bf4 contrast fix) — light
            // mode's info/success/warning/error/primary_400 are too pale
            // against bg_elevated when used directly as this foreground
            // (as low as 1.54:1); these are darker same-hue variants
            // reserved for the hint-key role, each ≥4.5:1 against
            // bg_elevated(198,200,210). The status/accent colors
            // themselves stay unchanged so their chip-background role
            // (fixed in this same task) is unaffected.
            hint_info: Color::Rgb(29, 87, 122),
            hint_success: Color::Rgb(24, 96, 48),
            hint_warning: Color::Rgb(113, 76, 27),
            hint_error: Color::Rgb(152, 46, 46),
            hint_accent: Color::Rgb(20, 92, 87),

            colors,
        }
    }
}

/// WCAG 2.x contrast-ratio guard (cas-7bf4).
///
/// Asserts the declared fg/bg token pairings that were audited and fixed for
/// the full-TUI contrast sweep. This is a REGRESSION GUARD, not a full
/// re-audit: it pins the specific pairings that were broken (status bar mode
/// chips, secondary/muted text, error text) against both theme modes so a
/// future palette edit can't silently reintroduce them.
///
/// Threshold: 4.5:1 for normal/readable text (WCAG AA), matching the AC.
/// Every pairing below was independently verified to render BELOW 4.5:1
/// before the cas-7bf4 fix (see task notes for the full audit table) — this
/// test would fail against the pre-fix palette.
#[cfg(test)]
mod contrast_guard {
    use super::*;
    use crate::ui::theme::colors::ColorPalette;

    const AA_NORMAL_TEXT: f64 = 4.5;

    fn channel_luminance(c: u8) -> f64 {
        let c = f64::from(c) / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }

    fn relative_luminance(color: Color) -> f64 {
        let Color::Rgb(r, g, b) = color else {
            panic!("contrast guard requires concrete RGB colors, got {color:?}");
        };
        0.2126 * channel_luminance(r)
            + 0.7152 * channel_luminance(g)
            + 0.0722 * channel_luminance(b)
    }

    /// WCAG contrast ratio between two colors, order-independent.
    fn contrast_ratio(a: Color, b: Color) -> f64 {
        let (l1, l2) = (relative_luminance(a), relative_luminance(b));
        let (lighter, darker) = if l1 >= l2 { (l1, l2) } else { (l2, l1) };
        (lighter + 0.05) / (darker + 0.05)
    }

    fn assert_pair(label: &str, fg: Color, bg: Color, min: f64) {
        let ratio = contrast_ratio(fg, bg);
        assert!(
            ratio >= min,
            "{label}: fg={fg:?} bg={bg:?} ratio={ratio:.2} below required {min:.1}:1"
        );
    }

    /// The chip/badge foreground must be legible against every saturated
    /// background it's actually painted on (status bar mode indicators,
    /// error banner, feedback selector, Cassy/TERM badges) in both theme
    /// modes. Before cas-7bf4 these used `text_primary`, which is
    /// near-white in dark mode and failed badly here (~1.7-2.1:1) — chip_fg
    /// is a fixed near-black chosen to work against both.
    #[test]
    fn chip_fg_passes_against_all_chip_backgrounds() {
        for (mode, colors, is_dark) in [
            ("dark", ColorPalette::dark(), true),
            ("light", ColorPalette::light(), false),
        ] {
            let p = Palette::from_colors(colors, is_dark);
            for (name, bg) in [
                ("status_info", p.status_info),
                ("status_warning", p.status_warning),
                ("status_success", p.status_success),
                ("accent", p.accent),
                ("accent_dim", p.accent_dim),
            ] {
                assert_pair(
                    &format!("{mode} chip_fg on {name}"),
                    p.chip_fg,
                    bg,
                    AA_NORMAL_TEXT,
                );
            }
        }
    }

    /// text_muted is rendered directly on bg_elevated (status bar hint
    /// suffixes, the [SUP]/branch segment). Before cas-7bf4: dark 3.36:1,
    /// light 2.52:1 — both below AA.
    #[test]
    fn text_muted_passes_against_bg_elevated() {
        for (mode, colors, is_dark) in [
            ("dark", ColorPalette::dark(), true),
            ("light", ColorPalette::light(), false),
        ] {
            let p = Palette::from_colors(colors, is_dark);
            assert_pair(
                &format!("{mode} text_muted on bg_elevated"),
                p.text_muted,
                p.bg_elevated,
                AA_NORMAL_TEXT,
            );
        }
    }

    /// status_error (text_error / disconnected-worker messaging) rendered
    /// on bg_elevated. Before cas-7bf4: dark 4.02:1 — below AA.
    #[test]
    fn status_error_passes_against_bg_elevated() {
        let p = Palette::from_colors(ColorPalette::dark(), true);
        assert_pair(
            "dark status_error on bg_elevated",
            p.status_error,
            p.bg_elevated,
            AA_NORMAL_TEXT,
        );
    }

    /// Sanity check that the guard itself can fail: a token pair known to
    /// be broken (the pre-fix chip pattern: text_primary fg on a chip bg in
    /// dark mode) must NOT pass at the AA threshold. This proves
    /// `assert_pair`/`contrast_ratio` aren't tautologically true.
    #[test]
    fn guard_catches_the_pre_fix_chip_pattern() {
        let p = Palette::from_colors(ColorPalette::dark(), true);
        let ratio = contrast_ratio(p.text_primary, p.status_info);
        assert!(
            ratio < AA_NORMAL_TEXT,
            "expected the pre-fix text_primary-on-status_info pattern to still read as broken \
             (ratio={ratio:.2}); if this now passes, the fixture no longer represents the bug"
        );
    }

    /// Keybind hint-key colors (status bar right-side hint row: ^P focus,
    /// ^Q quit, etc) rendered directly on bg_elevated. Before cas-7bf4 this
    /// used text_info/success/warning/error/accent directly — dark mode
    /// already cleared 4.5:1 (5.48-6.64), but light mode read as low as
    /// 1.54:1 since those tokens are also used as bright chip backgrounds
    /// elsewhere and can't be darkened without breaking that role. hint_*
    /// are the split-out, foreground-only variants.
    #[test]
    fn hint_key_colors_pass_against_bg_elevated_both_modes() {
        for (mode, colors, is_dark) in [
            ("dark", ColorPalette::dark(), true),
            ("light", ColorPalette::light(), false),
        ] {
            let p = Palette::from_colors(colors, is_dark);
            for (name, fg) in [
                ("hint_info", p.hint_info),
                ("hint_success", p.hint_success),
                ("hint_warning", p.hint_warning),
                ("hint_error", p.hint_error),
                ("hint_accent", p.hint_accent),
            ] {
                assert_pair(
                    &format!("{mode} {name} on bg_elevated"),
                    fg,
                    p.bg_elevated,
                    AA_NORMAL_TEXT,
                );
            }
        }
    }

    /// Falsification fixture: the pre-fix light-mode hint-key pattern
    /// (status_info/success/warning/accent used directly as a foreground on
    /// bg_elevated) must still read as broken, proving this guard isn't
    /// tautological.
    #[test]
    fn guard_catches_the_pre_fix_light_hint_pattern() {
        let p = Palette::from_colors(ColorPalette::light(), false);
        let ratio = contrast_ratio(p.accent, p.bg_elevated);
        assert!(
            ratio < 3.0,
            "expected the pre-fix accent-as-hint-key-on-bg_elevated pattern to still read as \
             broken (ratio={ratio:.2}); if this now passes, the fixture no longer represents \
             the bug"
        );
    }
}
