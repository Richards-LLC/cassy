//! Decode a source file to text, whatever byte-order mark it carries.
//!
//! GH #698: the symbol indexer read every candidate with `fs::read_to_string`,
//! so a UTF-16 file failed with "stream did not contain valid UTF-8" and was
//! counted as a permanent index failure. `cas index code` — the remediation
//! doctor itself printed — re-read the same bytes and failed identically, so
//! the warning could never clear.
//!
//! Two rules shape this module:
//!
//! * A file we *can* decode must be indexed, not failed. BOM-marked UTF-16
//!   (LE and BE) and UTF-8-with-BOM are ordinary text that a source tree can
//!   legitimately contain, particularly anything that has been through a
//!   Windows editor.
//! * A file we *cannot* decode is SKIPPED WITH A REASON, never guessed at.
//!   Lossy decoding would put replacement characters into the symbol index and
//!   call it success, which is worse than the failure it replaces: silent bad
//!   data outranks a loud error in how long it survives.

/// Why a file could not be turned into source text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SkipReason {
    /// Not valid UTF-8 and carrying no BOM that names another encoding.
    NotUtf8,
    /// BOM says UTF-16, but the byte count is odd — the file is truncated or
    /// is not really UTF-16.
    TruncatedUtf16,
    /// BOM says UTF-16 and the units are well-formed individually, but they do
    /// not form valid text (an unpaired surrogate).
    MalformedUtf16,
}

impl SkipReason {
    /// Operator-facing text. Named rather than numeric because this ends up in
    /// a doctor line that must explain itself without a lookup table.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::NotUtf8 => "not valid UTF-8 and no BOM identifies its encoding",
            Self::TruncatedUtf16 => "UTF-16 BOM but an odd byte count (truncated?)",
            Self::MalformedUtf16 => "UTF-16 BOM but malformed (unpaired surrogate)",
        }
    }
}

const BOM_UTF8: [u8; 3] = [0xEF, 0xBB, 0xBF];
const BOM_UTF16_LE: [u8; 2] = [0xFF, 0xFE];
const BOM_UTF16_BE: [u8; 2] = [0xFE, 0xFF];

/// Decode `bytes` to source text, honouring a leading byte-order mark.
///
/// The UTF-8 BOM is STRIPPED rather than passed through. `read_to_string`
/// accepted it (a BOM is valid UTF-8), so it reached the parser glued to the
/// first token — indexing a file whose first symbol is subtly wrong. That is a
/// quieter bug than the UTF-16 failure and was never reported, which is
/// precisely why it is worth removing here.
pub(crate) fn decode_source(bytes: &[u8]) -> Result<String, SkipReason> {
    if bytes.starts_with(&BOM_UTF8) {
        return String::from_utf8(bytes[BOM_UTF8.len()..].to_vec())
            .map_err(|_| SkipReason::NotUtf8);
    }
    if bytes.starts_with(&BOM_UTF16_LE) {
        return decode_utf16(&bytes[BOM_UTF16_LE.len()..], u16::from_le_bytes);
    }
    if bytes.starts_with(&BOM_UTF16_BE) {
        return decode_utf16(&bytes[BOM_UTF16_BE.len()..], u16::from_be_bytes);
    }
    // No BOM: plain UTF-8 or nothing we are willing to guess at. A source tree
    // in a legacy 8-bit encoding is a real thing, but inventing an encoding for
    // it would silently corrupt the index.
    String::from_utf8(bytes.to_vec()).map_err(|_| SkipReason::NotUtf8)
}

fn decode_utf16(body: &[u8], to_unit: fn([u8; 2]) -> u16) -> Result<String, SkipReason> {
    if !body.len().is_multiple_of(2) {
        return Err(SkipReason::TruncatedUtf16);
    }
    let units: Vec<u16> = body
        .chunks_exact(2)
        .map(|pair| to_unit([pair[0], pair[1]]))
        .collect();
    // Strict, never lossy: an unpaired surrogate is reported, not replaced with
    // U+FFFD and indexed as if it were the author's code.
    String::from_utf16(&units).map_err(|_| SkipReason::MalformedUtf16)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16_le(text: &str) -> Vec<u8> {
        let mut bytes = BOM_UTF16_LE.to_vec();
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes
    }

    fn utf16_be(text: &str) -> Vec<u8> {
        let mut bytes = BOM_UTF16_BE.to_vec();
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_be_bytes());
        }
        bytes
    }

    #[test]
    fn utf16_le_with_crlf_decodes() {
        // The reporter's exact shape: UTF-16 LE with CRLF line endings.
        let source = "export class AdminController {\r\n  ok() {}\r\n}\r\n";
        assert_eq!(decode_source(&utf16_le(source)).unwrap(), source);
    }

    #[test]
    fn utf16_be_decodes() {
        let source = "const answer = 42;\n";
        assert_eq!(decode_source(&utf16_be(source)).unwrap(), source);
    }

    #[test]
    fn a_utf8_bom_is_stripped_rather_than_left_on_the_first_token() {
        // The quiet half of GH #698: read_to_string accepted this, so the BOM
        // reached the parser attached to the first identifier. The decoded text
        // must start with the code, not with U+FEFF.
        let mut bytes = BOM_UTF8.to_vec();
        bytes.extend_from_slice(b"import fs from 'fs';\n");
        let decoded = decode_source(&bytes).unwrap();
        assert_eq!(decoded, "import fs from 'fs';\n");
        assert!(
            !decoded.starts_with('\u{feff}'),
            "a surviving BOM corrupts the first symbol the parser sees"
        );
    }

    #[test]
    fn plain_utf8_is_unchanged() {
        let source = "fn main() {}\n";
        assert_eq!(decode_source(source.as_bytes()).unwrap(), source);
    }

    #[test]
    fn a_binary_file_is_skipped_with_a_named_reason() {
        // A PNG header: no BOM, not UTF-8. Must be named, not guessed at.
        let bytes = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0xFF, 0x00];
        assert_eq!(decode_source(&bytes), Err(SkipReason::NotUtf8));
        assert!(SkipReason::NotUtf8.as_str().contains("not valid UTF-8"));
    }

    #[test]
    fn an_odd_byte_count_after_a_utf16_bom_is_truncated_not_lossy() {
        let mut bytes = utf16_le("ok");
        bytes.push(0x41); // one trailing byte: no longer whole UTF-16 units
        assert_eq!(decode_source(&bytes), Err(SkipReason::TruncatedUtf16));
    }

    #[test]
    fn an_unpaired_surrogate_is_reported_rather_than_replaced() {
        let mut bytes = BOM_UTF16_LE.to_vec();
        // A high surrogate with no low surrogate following it.
        bytes.extend_from_slice(&0xD800u16.to_le_bytes());
        bytes.extend_from_slice(&0x0041u16.to_le_bytes());
        assert_eq!(decode_source(&bytes), Err(SkipReason::MalformedUtf16));
    }

    #[test]
    fn an_empty_file_decodes_to_empty_text() {
        assert_eq!(decode_source(&[]).unwrap(), "");
        assert_eq!(decode_source(&BOM_UTF8).unwrap(), "");
        assert_eq!(decode_source(&BOM_UTF16_LE).unwrap(), "");
    }
}
