//! Shared display policy for terminal labels and copyable remedies.

/// Display-safety policy: Unicode 17 Default_Ignorable_Code_Point plus controls.
/// Pinned from https://www.unicode.org/Public/17.0.0/ucd/DerivedCoreProperties.txt.
/// These can hide or reorder a pasted ref even when shell quoting preserves argv.
pub fn is_unsafe_terminal_character(character: char) -> bool {
    character.is_control()
        || matches!(character,
            '\u{ad}' | '\u{34f}' | '\u{61c}' | '\u{115f}'..='\u{1160}'
            | '\u{17b4}'..='\u{17b5}' | '\u{180b}'..='\u{180f}'
            | '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}'
            | '\u{2060}'..='\u{206f}' | '\u{3164}' | '\u{fe00}'..='\u{fe0f}'
            | '\u{feff}' | '\u{ffa0}' | '\u{fff0}'..='\u{fff8}'
            | '\u{1bca0}'..='\u{1bca3}' | '\u{1d173}'..='\u{1d17a}'
            | '\u{e0000}'..='\u{e0fff}')
}
