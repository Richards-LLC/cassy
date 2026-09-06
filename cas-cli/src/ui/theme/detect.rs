//! Terminal background auto-detection for theme selection
//!
//! Detects whether the terminal has a dark or light background so Cassy
//! "just works" without explicit theme configuration.
//!
//! Detection priority:
//! 1. OSC 11 query — queries terminal background color via escape sequence
//! 2. COLORFGBG env var — set by some terminals (rxvt, some xterms)
//! 3. Default to dark — most developer terminals are dark

use std::io::{self, IsTerminal, Write};
use std::time::Duration;

use crossterm::event::{self, Event};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

use super::config::ThemeMode;

/// Detect the terminal's background brightness and return the appropriate theme mode.
///
/// This function does not consult the config file — callers should check
/// explicit config before calling this.
pub fn detect_background_theme() -> ThemeMode {
    // 1. Try OSC 11 terminal query
    if let Some(is_dark) = query_osc11_background() {
        return if is_dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        };
    }

    // 2. Try COLORFGBG env var
    if let Some(is_dark) = parse_colorfgbg() {
        return if is_dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        };
    }

    // 3. Default to dark
    ThemeMode::Dark
}

/// Query the terminal background color using OSC 11 escape sequence.
///
/// Sends `ESC ] 11 ; ? ST` and parses the RGB response.
/// Returns `Some(true)` for dark backgrounds, `Some(false)` for light,
/// or `None` if the terminal doesn't respond within the timeout.
///
/// Compatible terminals: iTerm2, Terminal.app, Kitty, Alacritty, WezTerm,
/// Windows Terminal, most xterm-compatible terminals.
fn query_osc11_background() -> Option<bool> {
    // Only works on a real TTY
    if !io::stdout().is_terminal() || !io::stdin().is_terminal() {
        return None;
    }

    // Enter raw mode to read the response character-by-character
    if enable_raw_mode().is_err() {
        return None;
    }

    let result = query_osc11_inner();

    // Always restore terminal state
    let _ = disable_raw_mode();

    result
}

/// Inner implementation of OSC 11 query (called with raw mode already enabled).
fn query_osc11_inner() -> Option<bool> {
    let mut stdout = io::stdout();

    // Send OSC 11 query: ESC ] 11 ; ? ST
    // Use BEL (\x07) as string terminator for broadest compatibility
    let query = b"\x1b]11;?\x07";
    if stdout.write_all(query).is_err() || stdout.flush().is_err() {
        return None;
    }

    // Read response with timeout. The response format is:
    //   ESC ] 11 ; rgb:RRRR/GGGG/BBBB ST
    // where each channel is 1-4 hex digits and ST is ESC \ or BEL
    let mut response = Vec::with_capacity(64);
    let deadline = Duration::from_millis(100);

    loop {
        if !event::poll(deadline).unwrap_or(false) {
            break; // Timeout — terminal doesn't support OSC 11
        }

        if let Ok(Event::Key(key_event)) = event::read() {
            // crossterm may deliver the response as key events in raw mode
            if let crossterm::event::KeyCode::Char(c) = key_event.code {
                response.push(c as u8);
            }
        }

        // Check if we've received the string terminator (BEL or ESC \)
        if response.ends_with(b"\x07") || response.ends_with(b"\x1b\\") {
            break;
        }

        // Safety limit
        if response.len() > 128 {
            break;
        }
    }

    // Drain any remaining buffered events (50ms grace period)
    while event::poll(Duration::from_millis(10)).unwrap_or(false) {
        let _ = event::read();
    }

    parse_osc11_response(&response)
}

/// Parse an OSC 11 response to determine if the background is dark.
///
/// Expected format: `...rgb:RRRR/GGGG/BBBB...` where channels are 1-4 hex digits.
/// Returns `Some(true)` for dark, `Some(false)` for light, `None` if unparseable.
fn parse_osc11_response(response: &[u8]) -> Option<bool> {
    let text = String::from_utf8_lossy(response);

    // Find "rgb:" prefix
    let rgb_start = text.find("rgb:")?;
    let rgb_part = &text[rgb_start + 4..];

    // Parse R/G/B channels separated by '/'
    let channels: Vec<&str> = rgb_part
        .split(['/', '\x07', '\x1b', '\\'])
        .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit()))
        .take(3)
        .collect();

    if channels.len() < 3 {
        return None;
    }

    // Parse hex values — normalize to 8-bit range
    let r = normalize_channel(channels[0])?;
    let g = normalize_channel(channels[1])?;
    let b = normalize_channel(channels[2])?;

    Some(is_dark_rgb(r, g, b))
}

/// Normalize a hex color channel value to 0-255 range.
///
/// OSC 11 can return 1, 2, or 4 hex digits per channel:
/// - 2 digits: already 8-bit (00-FF)
/// - 4 digits: 16-bit, take high byte
/// - 1 digit: scale to 8-bit
fn normalize_channel(hex: &str) -> Option<u8> {
    let val = u16::from_str_radix(hex, 16).ok()?;
    Some(match hex.len() {
        1 => (val * 17) as u8, // 0-F → 0-255 (multiply by 17)
        2 => val as u8,        // 00-FF → already 8-bit
        3 => (val >> 4) as u8, // 000-FFF → take high 8 bits
        4 => (val >> 8) as u8, // 0000-FFFF → take high byte
        _ => return None,
    })
}

/// Parse the COLORFGBG environment variable to detect dark/light background.
///
/// Format: "fg;bg" where values are ANSI color indices (0-15).
/// Background index >= 8 (or 0 for black) indicates dark.
/// Set by rxvt, some xterm configurations, and a few other terminals.
pub fn parse_colorfgbg() -> Option<bool> {
    let val = std::env::var("COLORFGBG").ok()?;
    parse_colorfgbg_value(&val)
}

/// Parse a COLORFGBG value string to determine if the background is dark.
///
/// Returns `Some(true)` for dark, `Some(false)` for light, `None` if unparseable.
fn parse_colorfgbg_value(val: &str) -> Option<bool> {
    let parts: Vec<&str> = val.split(';').collect();

    // Last element is the background color index
    let bg_str = parts.last()?;
    let bg: u8 = bg_str.parse().ok()?;

    // ANSI color indices: 0=black … 7=white, 8-15 the bright variants.
    // Vim's rule, which the terminals that set this variable were written
    // against: 0–6 and 8 (black, the dark hues, bright black) are dark; 7 and
    // 9–15 (white, the bright hues, bright white) are light. Konsole and rxvt
    // report a white background as `0;15`, which the old `bg >= 8` test
    // classified as dark and so painted near-white text on white (cas-4df0).
    Some(bg <= 6 || bg == 8)
}

/// Determine if an RGB color is "dark" using relative luminance.
///
/// Uses the ITU-R BT.709 luminance formula:
///   L = 0.2126*R + 0.7152*G + 0.0722*B
/// A threshold of 128 (50%) separates dark from light.
pub fn is_dark_rgb(r: u8, g: u8, b: u8) -> bool {
    let luminance = 0.2126 * (r as f64) + 0.7152 * (g as f64) + 0.0722 * (b as f64);
    luminance < 128.0
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // is_dark_rgb tests
    // ========================================================================

    #[test]
    fn test_black_is_dark() {
        assert!(is_dark_rgb(0, 0, 0));
    }

    #[test]
    fn test_white_is_light() {
        assert!(!is_dark_rgb(255, 255, 255));
    }

    #[test]
    fn test_typical_dark_terminal_bg() {
        // Common dark terminal backgrounds
        assert!(is_dark_rgb(30, 30, 30)); // Very dark gray
        assert!(is_dark_rgb(40, 44, 52)); // One Dark theme
        assert!(is_dark_rgb(29, 31, 33)); // Tomorrow Night
        assert!(is_dark_rgb(0, 43, 54)); // Solarized Dark
    }

    #[test]
    fn test_typical_light_terminal_bg() {
        // Common light terminal backgrounds
        assert!(!is_dark_rgb(253, 246, 227)); // Solarized Light
        assert!(!is_dark_rgb(255, 255, 255)); // Pure white
        assert!(!is_dark_rgb(245, 245, 245)); // Light gray
        assert!(!is_dark_rgb(250, 250, 250)); // Near-white
    }

    #[test]
    fn test_midtone_boundary() {
        // Around the 128 luminance boundary
        assert!(is_dark_rgb(100, 100, 100)); // Below midpoint
        assert!(!is_dark_rgb(180, 180, 180)); // Above midpoint
    }

    // ========================================================================
    // normalize_channel tests
    // ========================================================================

    #[test]
    fn test_normalize_2_digit() {
        assert_eq!(normalize_channel("FF"), Some(255));
        assert_eq!(normalize_channel("00"), Some(0));
        assert_eq!(normalize_channel("80"), Some(128));
        assert_eq!(normalize_channel("1A"), Some(26));
    }

    #[test]
    fn test_normalize_4_digit() {
        assert_eq!(normalize_channel("FFFF"), Some(255));
        assert_eq!(normalize_channel("0000"), Some(0));
        assert_eq!(normalize_channel("8080"), Some(128));
    }

    #[test]
    fn test_normalize_1_digit() {
        assert_eq!(normalize_channel("F"), Some(255));
        assert_eq!(normalize_channel("0"), Some(0));
        assert_eq!(normalize_channel("8"), Some(136));
    }

    #[test]
    fn test_normalize_3_digit() {
        assert_eq!(normalize_channel("FFF"), Some(255));
        assert_eq!(normalize_channel("000"), Some(0));
    }

    #[test]
    fn test_normalize_invalid() {
        assert_eq!(normalize_channel("GGGG"), None);
        assert_eq!(normalize_channel(""), None);
    }

    // ========================================================================
    // parse_osc11_response tests
    // ========================================================================

    #[test]
    fn test_parse_osc11_dark_background() {
        // iTerm2/Terminal.app format: 4-digit channels
        let response = b"\x1b]11;rgb:1E1E/1E1E/1E1E\x1b\\";
        assert_eq!(parse_osc11_response(response), Some(true));
    }

    #[test]
    fn test_parse_osc11_light_background() {
        // Solarized Light background
        let response = b"\x1b]11;rgb:FDF6/E3E3/DBDB\x1b\\";
        assert_eq!(parse_osc11_response(response), Some(false));
    }

    #[test]
    fn test_parse_osc11_2digit_channels() {
        // Some terminals use 2-digit channels
        let response = b"\x1b]11;rgb:1E/1E/1E\x07";
        assert_eq!(parse_osc11_response(response), Some(true));
    }

    #[test]
    fn test_parse_osc11_white_bg() {
        let response = b"\x1b]11;rgb:FFFF/FFFF/FFFF\x1b\\";
        assert_eq!(parse_osc11_response(response), Some(false));
    }

    #[test]
    fn test_parse_osc11_black_bg() {
        let response = b"\x1b]11;rgb:0000/0000/0000\x1b\\";
        assert_eq!(parse_osc11_response(response), Some(true));
    }

    #[test]
    fn test_parse_osc11_empty_response() {
        assert_eq!(parse_osc11_response(b""), None);
    }

    #[test]
    fn test_parse_osc11_garbage() {
        assert_eq!(parse_osc11_response(b"garbage data"), None);
    }

    #[test]
    fn test_parse_osc11_partial() {
        // Missing one channel
        assert_eq!(parse_osc11_response(b"\x1b]11;rgb:FF/FF\x07"), None);
    }

    // ========================================================================
    // parse_colorfgbg_value tests (pure function, no env mutation)
    // ========================================================================

    #[test]
    fn test_colorfgbg_dark_black_bg() {
        assert_eq!(parse_colorfgbg_value("15;0"), Some(true));
    }

    #[test]
    fn test_colorfgbg_dark_bright_bg() {
        assert_eq!(parse_colorfgbg_value("0;8"), Some(true));
    }

    #[test]
    fn test_colorfgbg_light_white_bg() {
        assert_eq!(parse_colorfgbg_value("0;7"), Some(false));
    }

    #[test]
    fn test_colorfgbg_light_bright_white_bg() {
        // Konsole and rxvt on a white background.
        assert_eq!(parse_colorfgbg_value("0;15"), Some(false));
    }

    #[test]
    fn test_colorfgbg_dark_hue_bg() {
        // A cyan background is a dark background under vim's rule.
        assert_eq!(parse_colorfgbg_value("0;6"), Some(true));
    }

    #[test]
    fn test_colorfgbg_three_part() {
        // Some terminals use "fg;extra;bg" format
        assert_eq!(parse_colorfgbg_value("15;default;0"), Some(true));
    }

    #[test]
    fn test_colorfgbg_invalid_format() {
        assert_eq!(parse_colorfgbg_value("invalid"), None);
    }

    #[test]
    fn test_colorfgbg_empty() {
        assert_eq!(parse_colorfgbg_value(""), None);
    }

    // ========================================================================
    // detect_background_theme tests (require env mutation, serialized)
    // ========================================================================

    #[test]
    fn test_detect_defaults_to_dark() {
        let _guard =
            crate::test_support::TestEnvGuard::with_optional_vars(&[("COLORFGBG", None)]);
        // In test environment (non-TTY, no COLORFGBG), should default to dark
        let mode = detect_background_theme();
        assert_eq!(mode, ThemeMode::Dark);
    }

    #[test]
    fn test_detect_uses_colorfgbg_when_available() {
        let _guard =
            crate::test_support::TestEnvGuard::with_vars(&[("COLORFGBG", "0;7")]);
        let mode = detect_background_theme();
        // In non-TTY test, OSC 11 won't work, so COLORFGBG should be used
        assert_eq!(mode, ThemeMode::Light);
    }
}
