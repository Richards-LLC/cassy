//! Keystream / bracketed-paste classification for prompt-submit detection.
//!
//! Terminal clients may deliver raw bracketed paste (`\x1b[200~` … `\x1b[201~`)
//! as a byte stream. Embedded CR/LF inside that region are literal paste content
//! and must not mark a prompt submit (cas-4b99). Only CR/LF outside paste
//! delimiters are submit transitions. Delimiters may be split across packets.

/// Streaming bracketed-paste detector for keystream submit classification (cas-4b99).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct BracketedPasteTracker {
    state: PasteTrackState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum PasteTrackState {
    /// Outside paste. `partial` = matched prefix length of [`PASTE_START`].
    Normal { partial: u8 },
    /// Inside paste (including after start delimiter). `partial` = matched
    /// prefix length of [`PASTE_END`].
    InPaste { partial: u8 },
}

impl Default for PasteTrackState {
    fn default() -> Self {
        Self::Normal { partial: 0 }
    }
}

const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";

/// Classification of one keystream byte for submit side effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamByteClass {
    /// Keyboard / non-paste byte. CR/LF here is a prompt submit.
    Key,
    /// Inside bracketed paste (delimiters + payload). Never a submit.
    Paste,
}

impl BracketedPasteTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the tracker is currently inside a bracketed paste region
    /// (after a complete start delimiter, before a complete end delimiter).
    pub fn in_paste(&self) -> bool {
        matches!(self.state, PasteTrackState::InPaste { .. })
    }

    /// Feed one byte; returns whether it is paste content for submit purposes.
    ///
    /// Forwarding is always byte-exact — callers still write the same bytes;
    /// this only classifies submit eligibility.
    pub fn feed_byte(&mut self, b: u8) -> StreamByteClass {
        match self.state {
            PasteTrackState::Normal { partial } => self.feed_normal(partial, b),
            PasteTrackState::InPaste { partial } => self.feed_in_paste(partial, b),
        }
    }

    /// Feed a chunk and return whether any Key-class CR/LF occurred.
    pub fn feed_chunk_marks_submit(&mut self, data: &[u8]) -> bool {
        let mut marks = false;
        for &b in data {
            let class = self.feed_byte(b);
            if class == StreamByteClass::Key && matches!(b, b'\r' | b'\n') {
                marks = true;
            }
        }
        marks
    }

    fn feed_normal(&mut self, partial: u8, b: u8) -> StreamByteClass {
        let p = partial as usize;
        if p < PASTE_START.len() && b == PASTE_START[p] {
            let next = p + 1;
            if next == PASTE_START.len() {
                self.state = PasteTrackState::InPaste { partial: 0 };
                // Start delimiter completes — treat as paste framing.
                StreamByteClass::Paste
            } else {
                self.state = PasteTrackState::Normal {
                    partial: next as u8,
                };
                // Partial start match is not CR/LF; class is irrelevant for submit.
                // Use Key so a failed match never suppresses a later real Enter.
                StreamByteClass::Key
            }
        } else if p > 0 {
            // Partial start failed — restart matching on this byte.
            self.state = PasteTrackState::Normal { partial: 0 };
            self.feed_byte(b)
        } else {
            StreamByteClass::Key
        }
    }

    fn feed_in_paste(&mut self, partial: u8, b: u8) -> StreamByteClass {
        let p = partial as usize;
        if p < PASTE_END.len() && b == PASTE_END[p] {
            let next = p + 1;
            if next == PASTE_END.len() {
                self.state = PasteTrackState::Normal { partial: 0 };
            } else {
                self.state = PasteTrackState::InPaste {
                    partial: next as u8,
                };
            }
            StreamByteClass::Paste
        } else if p > 0 {
            // Partial end failed — still inside paste; restart end match on `b`.
            self.state = PasteTrackState::InPaste { partial: 0 };
            if b == PASTE_END[0] {
                self.state = PasteTrackState::InPaste { partial: 1 };
            }
            StreamByteClass::Paste
        } else if b == PASTE_END[0] {
            self.state = PasteTrackState::InPaste { partial: 1 };
            StreamByteClass::Paste
        } else {
            StreamByteClass::Paste
        }
    }
}

/// Whether a keystream chunk contains a prompt-submit transition, respecting
/// bracketed-paste regions (fresh tracker — single complete chunk).
pub fn key_stream_marks_submit(data: &[u8]) -> bool {
    if data.is_empty() {
        return false;
    }
    BracketedPasteTracker::new().feed_chunk_marks_submit(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lone_enter_marks_submit_cas_4b99() {
        assert!(key_stream_marks_submit(b"\r"));
        assert!(key_stream_marks_submit(b"\n"));
        assert!(key_stream_marks_submit(b"\r\n"));
        assert!(key_stream_marks_submit(b"hello\r"));
        assert!(!key_stream_marks_submit(b"hello"));
        assert!(!key_stream_marks_submit(b""));
    }

    /// AC1: Embedded CR/LF inside bracketed paste does not mark prompt submit.
    #[test]
    fn embedded_crlf_inside_paste_does_not_mark_submit_cas_4b99() {
        let paste = b"\x1b[200~line1\nline2\r\nline3\r\x1b[201~";
        assert!(
            !key_stream_marks_submit(paste),
            "embedded newlines in paste must not submit"
        );
        assert!(!key_stream_marks_submit(
            b"\x1b[200~path/with\nnewline\r\x1b[201~"
        ));
        assert!(!key_stream_marks_submit(b"\x1b[200~\x1b[201~"));
    }

    /// AC2: Enter after paste terminator does mark submit.
    #[test]
    fn enter_after_paste_terminator_marks_submit_cas_4b99() {
        let mut t = BracketedPasteTracker::new();
        assert!(!t.feed_chunk_marks_submit(b"\x1b[200~hello\nworld\x1b[201~"));
        assert!(
            t.feed_chunk_marks_submit(b"\r"),
            "Enter after paste end must submit"
        );

        // Same as one chunk: paste then Enter.
        assert!(key_stream_marks_submit(
            b"\x1b[200~multi\nline\x1b[201~\r"
        ));
        assert!(key_stream_marks_submit(
            b"\x1b[200~multi\nline\x1b[201~\n"
        ));
    }

    /// AC3: Split delimiter packets are handled.
    #[test]
    fn split_delimiter_packets_cas_4b99() {
        let mut t = BracketedPasteTracker::new();
        // Split start: \x1b[200 / ~
        assert!(!t.feed_chunk_marks_submit(b"\x1b[200"));
        assert!(!t.in_paste());
        assert!(!t.feed_chunk_marks_submit(b"~"));
        assert!(t.in_paste(), "complete start must enter paste");

        // Content with newlines while split across packets.
        assert!(!t.feed_chunk_marks_submit(b"line1\n"));
        assert!(!t.feed_chunk_marks_submit(b"line2\r"));
        assert!(t.in_paste());

        // Split end: \x1b[20 / 1~
        assert!(!t.feed_chunk_marks_submit(b"\x1b[20"));
        assert!(t.in_paste());
        assert!(!t.feed_chunk_marks_submit(b"1~"));
        assert!(!t.in_paste(), "complete end must leave paste");

        assert!(t.feed_chunk_marks_submit(b"\r"));
    }

    /// AC3 variant: single-byte feeds across full paste + Enter.
    #[test]
    fn per_byte_stream_paste_then_enter_cas_4b99() {
        let mut t = BracketedPasteTracker::new();
        let stream = b"\x1b[200~a\nb\rc\x1b[201~\r";
        let mut submit_at = Vec::new();
        for (i, &b) in stream.iter().enumerate() {
            let class = t.feed_byte(b);
            if class == StreamByteClass::Key && matches!(b, b'\r' | b'\n') {
                submit_at.push(i);
            }
        }
        // Only the final CR (after 201~) is a submit.
        assert_eq!(
            submit_at,
            vec![stream.len() - 1],
            "only trailing Enter outside paste should submit, got {submit_at:?}"
        );
    }

    /// AC4: Classification is non-destructive — every input byte is accounted for
    /// (forwarding remains caller's responsibility and is byte-exact).
    #[test]
    fn every_byte_classified_byte_exact_coverage_cas_4b99() {
        let payload = b"\x1b[200~hello\r\nworld\x1b[201~\rxyz";
        let mut t = BracketedPasteTracker::new();
        let mut classes = Vec::with_capacity(payload.len());
        for &b in payload {
            classes.push(t.feed_byte(b));
        }
        assert_eq!(classes.len(), payload.len());
        // Start delimiter (6) + payload + end delimiter (6) = Paste; trailing Enter = Key submit;
        // xyz = Key non-submit.
        let start_len = PASTE_START.len();
        let end_len = PASTE_END.len();
        let paste_body = b"hello\r\nworld";
        let paste_region = start_len + paste_body.len() + end_len;
        for (i, c) in classes.iter().enumerate().take(paste_region) {
            // After complete start, all through end are Paste. Partial start bytes
            // before completion are Key (non-CR).
            if i + 1 >= start_len {
                assert_eq!(
                    *c,
                    StreamByteClass::Paste,
                    "byte {i} ({:#x}) should be Paste",
                    payload[i]
                );
            }
        }
        assert_eq!(classes[paste_region], StreamByteClass::Key); // \r submit
        assert_eq!(classes[paste_region + 1], StreamByteClass::Key); // x
    }

    #[test]
    fn false_start_then_real_enter_still_submits_cas_4b99() {
        // Partial CSI that is not paste start, then Enter.
        assert!(key_stream_marks_submit(b"\x1b[1m\r"));
        // ESC then Enter
        assert!(key_stream_marks_submit(b"\x1b\r"));
    }

    #[test]
    fn content_resembling_end_delimiter_stays_in_paste_cas_4b99() {
        let mut t = BracketedPasteTracker::new();
        t.feed_chunk_marks_submit(b"\x1b[200~");
        // Content that looks like end but has wrong digit
        assert!(!t.feed_chunk_marks_submit(b"\x1b[202~still\npaste"));
        assert!(t.in_paste());
        assert!(!t.feed_chunk_marks_submit(b"\x1b[201~"));
        assert!(!t.in_paste());
    }
}
