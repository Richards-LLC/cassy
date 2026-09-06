//! Base color definitions for Cassy themes

use ratatui::style::Color;

/// Base color palette - inspired by Linear/Raycast muted aesthetics
#[derive(Debug, Clone)]
pub struct ColorPalette {
    // Grayscale
    pub gray_50: Color,
    pub gray_100: Color,
    pub gray_200: Color,
    pub gray_300: Color,
    pub gray_400: Color,
    pub gray_500: Color,
    pub gray_600: Color,
    pub gray_700: Color,
    pub gray_800: Color,
    pub gray_900: Color,

    // Primary accent (teal - ops console)
    pub primary_100: Color,
    pub primary_200: Color,
    pub primary_300: Color,
    pub primary_400: Color,
    pub primary_500: Color,

    // Status colors
    pub success: Color,
    pub success_dim: Color,
    pub warning: Color,
    pub warning_dim: Color,
    pub error: Color,
    pub error_dim: Color,
    pub info: Color,
    pub info_dim: Color,

    // Specialty colors
    pub purple: Color,
    pub purple_dim: Color,
    pub cyan: Color,
    pub cyan_dim: Color,
    pub orange: Color,
    pub orange_dim: Color,
}

impl ColorPalette {
    /// Linear-inspired dark theme
    pub fn dark() -> Self {
        Self {
            // Modern muted grays (slightly blue-tinted)
            gray_50: Color::Rgb(250, 250, 252),
            gray_100: Color::Rgb(228, 229, 235),
            gray_200: Color::Rgb(198, 200, 210),
            gray_300: Color::Rgb(160, 162, 175),
            // cas-7bf4: lightened from (120,123,138) — text_muted/task_open/
            // priority_low/status_neutral all read at <4.5:1 against
            // bg_elevated (40,43,53); this hits ~4.6:1 while staying dimmer
            // than gray_300.
            gray_400: Color::Rgb(144, 147, 159),
            gray_500: Color::Rgb(90, 93, 107),
            gray_600: Color::Rgb(60, 63, 75),
            gray_700: Color::Rgb(40, 43, 53),
            gray_800: Color::Rgb(28, 30, 38),
            gray_900: Color::Rgb(18, 19, 24),

            // Teal accent (ops console)
            primary_100: Color::Rgb(160, 240, 230),
            primary_200: Color::Rgb(120, 220, 210),
            primary_300: Color::Rgb(80, 200, 190),
            primary_400: Color::Rgb(40, 180, 170),
            primary_500: Color::Rgb(30, 140, 135),

            // Status - muted for dark theme
            success: Color::Rgb(80, 200, 120),
            success_dim: Color::Rgb(40, 100, 60),
            warning: Color::Rgb(240, 160, 60),
            warning_dim: Color::Rgb(120, 80, 30),
            // cas-7bf4: lightened from (230,90,90) — status_error/text_error
            // read at 4.02:1 against bg_elevated (40,43,53), just under the
            // 4.5:1 floor; this hits ~4.6:1.
            error: Color::Rgb(233, 108, 108),
            error_dim: Color::Rgb(115, 45, 45),
            info: Color::Rgb(70, 170, 230),
            info_dim: Color::Rgb(35, 85, 115),

            // Specialty colors
            purple: Color::Rgb(180, 130, 255),
            purple_dim: Color::Rgb(90, 65, 128),
            cyan: Color::Rgb(80, 200, 210),
            cyan_dim: Color::Rgb(40, 100, 105),
            orange: Color::Rgb(230, 150, 80),
            orange_dim: Color::Rgb(115, 75, 40),
        }
    }

    /// Light theme variant
    pub fn light() -> Self {
        Self {
            // Inverted grays for light background
            gray_50: Color::Rgb(18, 19, 24),
            gray_100: Color::Rgb(28, 30, 38),
            gray_200: Color::Rgb(40, 43, 53),
            gray_300: Color::Rgb(60, 63, 75),
            // cas-7bf4: darkened from (90,93,107) — status_neutral/task_open/
            // rule_draft read at <3:1 against bg_elevated (198,200,210);
            // this hits ~4.6:1.
            gray_400: Color::Rgb(80, 83, 95),
            // cas-7bf4: darkened from (120,123,138) — text_muted read at
            // 2.52:1 against bg_elevated (198,200,210); this hits ~4.7:1.
            gray_500: Color::Rgb(80, 82, 92),
            gray_600: Color::Rgb(160, 162, 175),
            gray_700: Color::Rgb(198, 200, 210),
            gray_800: Color::Rgb(228, 229, 235),
            gray_900: Color::Rgb(250, 250, 252),

            // Same accent colors (teal)
            primary_100: Color::Rgb(160, 240, 230),
            primary_200: Color::Rgb(120, 220, 210),
            primary_300: Color::Rgb(80, 200, 190),
            primary_400: Color::Rgb(40, 180, 170),
            primary_500: Color::Rgb(30, 140, 135),

            // Meaning colours for a light background (cas-4df0). Each sits in
            // the narrow luminance band that reads as a mark (≥ 3:1) on white
            // and on Solarized light (#fdf6e3) while still carrying the
            // near-black chip foreground at ≥ 4.5:1 (the contrast guard in
            // palette.rs). Text never uses them: a verdict word is bold in the
            // terminal's own foreground. The previous warning measured 2.8:1
            // on white.
            success: Color::Rgb(30, 150, 75),
            success_dim: Color::Rgb(200, 240, 210),
            warning: Color::Rgb(176, 112, 16),
            warning_dim: Color::Rgb(255, 245, 200),
            error: Color::Rgb(200, 60, 60),
            error_dim: Color::Rgb(255, 220, 220),
            info: Color::Rgb(40, 130, 200),
            info_dim: Color::Rgb(220, 235, 255),

            purple: Color::Rgb(130, 80, 200),
            purple_dim: Color::Rgb(240, 230, 255),
            cyan: Color::Rgb(40, 160, 170),
            cyan_dim: Color::Rgb(220, 250, 252),
            orange: Color::Rgb(200, 120, 50),
            orange_dim: Color::Rgb(255, 240, 220),
        }
    }

    /// Minions theme variant - yellow primary, denim blue secondary
    pub fn minions(is_dark: bool) -> Self {
        let base = if is_dark { Self::dark() } else { Self::light() };
        Self {
            // Override primary accent from teal to Minion yellow
            primary_100: Color::Rgb(255, 245, 157), // Light banana
            primary_200: Color::Rgb(255, 235, 59),  // Bright yellow
            primary_300: Color::Rgb(255, 213, 0),   // Minion yellow
            primary_400: Color::Rgb(255, 193, 7),   // Amber accent
            primary_500: Color::Rgb(255, 160, 0),   // Deep amber

            // Override info to denim blue (overalls)
            info: Color::Rgb(96, 140, 245),     // Bright denim blue (WCAG AA)
            info_dim: Color::Rgb(48, 70, 123),  // Dark denim

            // Override cyan to goggle silver
            cyan: Color::Rgb(192, 200, 210),    // Goggle silver
            cyan_dim: Color::Rgb(96, 100, 105), // Dark goggle

            // Keep everything else from the base
            ..base
        }
    }

    /// Tokyo Night theme variant
    ///
    /// Based on the official Tokyo Night color palette by enkia.
    /// Uses the Storm variant for backgrounds (#24283b base) with full
    /// Tokyo Night syntax colors: blue-purple primary, warm yellow warning,
    /// pink-red error, soft green success, and the signature cyan blue.
    pub fn tokyo_night(_is_dark: bool) -> Self {
        Self {
            // Tokyo Night Storm backgrounds — replace the blue-gray Linear grays
            // with the actual TN background ramp
            // gray_900 = deepest bg (#1a1b26 night / #16161e storm floor)
            // gray_800 = secondary bg (#24283b storm)
            // gray_700 = elevated surface (#292e42 dark5)
            // gray_600 = border accent (#3b4261 dark3)
            // gray_500 = terminal_black / muted border (#414868)
            // gray_400 = comment / muted text (#565f89)
            // gray_300 = fg secondary (#a9b1d6)
            // gray_200 = fg primary step down (#cdd6f4 approx)
            // gray_100 = fg primary (#c0caf5)
            // gray_50  = bright white fg (#d5d6db)
            gray_50:  Color::Rgb(213, 214, 219), // #d5d6db — bright fg
            gray_100: Color::Rgb(192, 202, 245), // #c0caf5 — fg
            gray_200: Color::Rgb(169, 177, 214), // #a9b1d6 — fg secondary
            gray_300: Color::Rgb(122, 131, 174), // #7a83ae — fg tertiary
            gray_400: Color::Rgb(86, 95, 137),   // #565f89 — comment / muted
            gray_500: Color::Rgb(65, 72, 104),   // #414868 — terminal_black
            gray_600: Color::Rgb(59, 66, 97),    // #3b4261 — dark3 / border
            gray_700: Color::Rgb(41, 46, 66),    // #292e42 — dark5 / elevated
            gray_800: Color::Rgb(36, 40, 59),    // #24283b — storm bg secondary
            gray_900: Color::Rgb(26, 27, 38),    // #1a1b26 — night bg primary

            // Primary accent: Tokyo Night blue (#7aa2f7)
            // Ramp from lightest tint down to the deep blue0 (#3d59a1)
            primary_100: Color::Rgb(199, 215, 254), // pale blue tint
            primary_200: Color::Rgb(158, 190, 252), // #9ebefc — lighter blue
            primary_300: Color::Rgb(122, 162, 247), // #7aa2f7 — blue (canonical)
            primary_400: Color::Rgb(97, 132, 220),  // #6184dc — mid blue
            primary_500: Color::Rgb(61, 89, 161),   // #3d59a1 — blue0 / dim accent

            // Status colors — canonical Tokyo Night syntax
            success:     Color::Rgb(158, 206, 106), // #9ece6a — green
            success_dim: Color::Rgb(65, 166, 181),  // #41a6b5 — git.add teal (dim)
            warning:     Color::Rgb(224, 175, 104), // #e0af68 — yellow
            warning_dim: Color::Rgb(112, 87, 52),   // half-luminance yellow-brown
            error:       Color::Rgb(247, 118, 142), // #f7768e — red
            error_dim:   Color::Rgb(145, 76, 84),   // #914c54 — git.delete (dim)
            info:        Color::Rgb(187, 154, 247), // #bb9af7 — magenta / purple
            info_dim:    Color::Rgb(86, 67, 130),   // muted purple shadow

            // Specialty colors — purple shifted lighter to differentiate from info
            purple:     Color::Rgb(207, 174, 247), // #cfaef7 — lighter magenta (distinct from info)
            purple_dim: Color::Rgb(103, 87, 155),  // deep purple shadow
            cyan:       Color::Rgb(125, 207, 255), // #7dcfff — cyan
            cyan_dim:   Color::Rgb(42, 195, 222),  // #2ac3de — blue1 (brighter cyan)
            orange:     Color::Rgb(255, 158, 100), // #ff9e64 — orange
            orange_dim: Color::Rgb(127, 79, 50),   // muted orange
        }
    }

    /// High contrast accessibility variant
    pub fn high_contrast() -> Self {
        Self {
            gray_50: Color::Rgb(255, 255, 255),
            gray_100: Color::Rgb(255, 255, 255),
            gray_200: Color::Rgb(220, 220, 220),
            gray_300: Color::Rgb(180, 180, 180),
            gray_400: Color::Rgb(140, 140, 140),
            gray_500: Color::Rgb(100, 100, 100),
            gray_600: Color::Rgb(60, 60, 60),
            gray_700: Color::Rgb(40, 40, 40),
            gray_800: Color::Rgb(20, 20, 20),
            gray_900: Color::Rgb(0, 0, 0),

            primary_100: Color::Rgb(255, 255, 100),
            primary_200: Color::Rgb(255, 255, 80),
            primary_300: Color::Rgb(255, 255, 60),
            primary_400: Color::Rgb(255, 255, 0),
            primary_500: Color::Rgb(200, 200, 0),

            success: Color::Rgb(0, 255, 0),
            success_dim: Color::Rgb(0, 100, 0),
            warning: Color::Rgb(255, 255, 0),
            warning_dim: Color::Rgb(100, 100, 0),
            error: Color::Rgb(255, 50, 50),
            error_dim: Color::Rgb(100, 0, 0),
            info: Color::Rgb(0, 200, 255),
            info_dim: Color::Rgb(0, 80, 100),

            purple: Color::Rgb(255, 0, 255),
            purple_dim: Color::Rgb(100, 0, 100),
            cyan: Color::Rgb(0, 255, 255),
            cyan_dim: Color::Rgb(0, 100, 100),
            orange: Color::Rgb(255, 165, 0),
            orange_dim: Color::Rgb(100, 65, 0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    // ── Tokyo Night tests ────────────────────────────────────────────────────

    #[test]
    fn tokyo_night_primary_is_blue_not_teal() {
        let tn = ColorPalette::tokyo_night(true);
        match tn.primary_300 {
            // #7aa2f7 = (122, 162, 247) — blue must dominate red and green
            Color::Rgb(r, g, b) => {
                assert!(b > r, "primary_300 blue ({b}) should exceed red ({r})");
                assert!(b > g, "primary_300 blue ({b}) should exceed green ({g})");
                assert!(b > 200, "primary_300 should be a strong blue, got {b}");
            }
            other => panic!("Expected RGB color, got {other:?}"),
        }
    }

    #[test]
    fn tokyo_night_error_is_pink_red() {
        let tn = ColorPalette::tokyo_night(true);
        match tn.error {
            // #f7768e = (247, 118, 142) — red dominant, significant blue (pink cast)
            Color::Rgb(r, g, b) => {
                assert!(r > 200, "error red component should be high for #f7768e, got {r}");
                assert!(r > g, "error should be red-dominant, got r={r} g={g}");
                assert!(b > g, "error should have pink cast (b > g), got b={b} g={g}");
            }
            other => panic!("Expected RGB color, got {other:?}"),
        }
    }

    #[test]
    fn tokyo_night_success_is_soft_green() {
        let tn = ColorPalette::tokyo_night(true);
        match tn.success {
            // #9ece6a = (158, 206, 106) — green dominant
            Color::Rgb(r, g, b) => {
                assert!(g > r, "success green ({g}) should exceed red ({r})");
                assert!(g > b, "success green ({g}) should exceed blue ({b})");
                assert!(g > 180, "success should be a visible green, got {g}");
            }
            other => panic!("Expected RGB color, got {other:?}"),
        }
    }

    #[test]
    fn tokyo_night_warning_is_warm_yellow() {
        let tn = ColorPalette::tokyo_night(true);
        match tn.warning {
            // #e0af68 = (224, 175, 104) — red+green dominant (yellow), low blue
            Color::Rgb(r, g, b) => {
                assert!(r > b, "warning red ({r}) should exceed blue ({b})");
                assert!(g > b, "warning green ({g}) should exceed blue ({b})");
                assert!(r > 180, "warning should have strong red for warm yellow, got {r}");
            }
            other => panic!("Expected RGB color, got {other:?}"),
        }
    }

    #[test]
    fn tokyo_night_bg_is_dark_navy() {
        let tn = ColorPalette::tokyo_night(true);
        match tn.gray_900 {
            // #1a1b26 = (26, 27, 38) — very dark, blue tinted
            Color::Rgb(r, g, b) => {
                assert!(b > r, "bg blue ({b}) should exceed red ({r}) for navy tint");
                assert!(r < 40, "bg should be very dark, red={r}");
                assert!(g < 40, "bg should be very dark, green={g}");
            }
            other => panic!("Expected RGB color, got {other:?}"),
        }
    }

    #[test]
    fn tokyo_night_cyan_is_sky_blue() {
        let tn = ColorPalette::tokyo_night(true);
        match tn.cyan {
            // #7dcfff = (125, 207, 255) — blue dominant, cyan-ish
            Color::Rgb(r, _g, b) => {
                assert!(b > r, "cyan blue ({b}) should exceed red ({r})");
                assert!(b > 200, "cyan should be a bright sky blue, got {b}");
            }
            other => panic!("Expected RGB color, got {other:?}"),
        }
    }

    #[test]
    fn tokyo_night_differs_from_dark_base() {
        let dark = ColorPalette::dark();
        let tn = ColorPalette::tokyo_night(true);
        assert_ne!(tn.primary_300, dark.primary_300, "primary accent should differ");
        assert_ne!(tn.error, dark.error, "error color should differ");
        assert_ne!(tn.gray_900, dark.gray_900, "bg should differ from Linear base");
    }

    #[test]
    fn tokyo_night_purple_is_info() {
        let tn = ColorPalette::tokyo_night(true);
        match tn.info {
            // #bb9af7 = (187, 154, 247) — blue+red mix = purple/magenta
            Color::Rgb(r, g, b) => {
                assert!(b > g, "info blue ({b}) should exceed green ({g}) for purple");
                assert!(r > g, "info red ({r}) should exceed green ({g}) for purple");
                assert!(b > 200, "info should be a visible purple-blue, got {b}");
            }
            other => panic!("Expected RGB color, got {other:?}"),
        }
    }

    // ── Minions tests ────────────────────────────────────────────────────────

    #[test]
    fn minions_palette_has_yellow_primary() {
        let minions = ColorPalette::minions(true);
        match minions.primary_300 {
            Color::Rgb(r, g, _) => {
                assert!(r > 200, "primary_300 red should be bright yellow, got {r}");
                assert!(g > 150, "primary_300 green should be bright yellow, got {g}");
            }
            other => panic!("Expected RGB color, got {other:?}"),
        }
    }

    #[test]
    fn minions_palette_has_denim_blue_info() {
        let minions = ColorPalette::minions(true);
        match minions.info {
            Color::Rgb(r, _, b) => {
                assert!(b > r, "info blue should exceed red for denim blue");
                assert!(b > 150, "info blue component should be strong, got {b}");
            }
            other => panic!("Expected RGB color, got {other:?}"),
        }
    }

    #[test]
    fn minions_palette_differs_from_dark() {
        let dark = ColorPalette::dark();
        let minions = ColorPalette::minions(true);
        assert_ne!(minions.primary_300, dark.primary_300, "primary should differ");
        assert_ne!(minions.info, dark.info, "info should differ");
    }

    #[test]
    fn minions_palette_preserves_base_bg() {
        let dark = ColorPalette::dark();
        let minions = ColorPalette::minions(true);
        assert_eq!(minions.gray_900, dark.gray_900, "bg should inherit from dark base");
    }
}
