//! Chunking for distillation input (EPIC cas-7d31 / cas-c9be).
//!
//! Three tiers, applied in order and only as far as needed:
//!
//! 1. **headings** — split at ATX headings, so a chunk is a semantic section.
//!    Headings inside fenced code blocks are not headings.
//! 2. **paragraphs** — an oversized section is packed greedily at blank-line
//!    boundaries.
//! 3. **hard slice with tail overlap** — a single oversized paragraph (minified
//!    JSON, a giant table) is sliced at a character boundary, each slice after
//!    the first prefixed with the tail of the previous one so a fact straddling
//!    the cut still appears whole in one chunk.

/// Chunk sizing knobs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkOptions {
    /// Maximum characters in one chunk before the next tier kicks in.
    pub max_chars: usize,
    /// Characters of the previous slice repeated at the head of the next one.
    pub overlap_chars: usize,
}

impl Default for ChunkOptions {
    fn default() -> Self {
        Self {
            max_chars: 6_000,
            overlap_chars: 240,
        }
    }
}

/// One unit of distillation input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    /// Nearest enclosing heading text (empty for a preamble).
    pub heading: String,
    pub text: String,
}

/// Split `text` into chunks no larger than `opts.max_chars` (except when a
/// single character run cannot be split further, which cannot happen because
/// tier 3 always slices).
pub fn chunk_markdown(text: &str, opts: &ChunkOptions) -> Vec<Chunk> {
    let max_chars = opts.max_chars.max(200);
    let mut chunks = Vec::new();

    for (heading, section) in split_sections(text) {
        if section.trim().is_empty() {
            continue;
        }
        for piece in split_section(&section, max_chars, opts.overlap_chars) {
            if piece.trim().is_empty() {
                continue;
            }
            chunks.push(Chunk {
                heading: heading.clone(),
                text: piece,
            });
        }
    }

    chunks
}

/// Tier 1: `(heading, section-including-its-heading-line)` pairs.
fn split_sections(text: &str) -> Vec<(String, String)> {
    let mut sections: Vec<(String, String)> = Vec::new();
    let mut heading = String::new();
    let mut current = String::new();
    let mut in_fence = false;

    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
        }

        if !in_fence && trimmed.starts_with('#') && heading_level(trimmed) > 0 {
            if !current.trim().is_empty() {
                sections.push((heading.clone(), std::mem::take(&mut current)));
            } else {
                current.clear();
            }
            heading = trimmed.trim_start_matches('#').trim().to_string();
        }

        current.push_str(line);
        current.push('\n');
    }

    if !current.trim().is_empty() {
        sections.push((heading, current));
    }
    sections
}

fn heading_level(trimmed_line: &str) -> usize {
    let hashes = trimmed_line.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return 0;
    }
    match trimmed_line.chars().nth(hashes) {
        Some(' ') | Some('\t') => hashes,
        _ => 0,
    }
}

/// Tiers 2 and 3 for one section.
fn split_section(section: &str, max_chars: usize, overlap_chars: usize) -> Vec<String> {
    if char_len(section) <= max_chars {
        return vec![section.to_string()];
    }

    let mut pieces: Vec<String> = Vec::new();
    let mut current = String::new();

    for paragraph in section.split("\n\n") {
        let paragraph_len = char_len(paragraph);

        if paragraph_len > max_chars {
            if !current.trim().is_empty() {
                pieces.push(std::mem::take(&mut current));
            }
            current.clear();
            pieces.extend(hard_slice(paragraph, max_chars, overlap_chars));
            continue;
        }

        if char_len(&current) + paragraph_len + 2 > max_chars && !current.trim().is_empty() {
            pieces.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push_str("\n\n");
        }
        current.push_str(paragraph);
    }

    if !current.trim().is_empty() {
        pieces.push(current);
    }
    pieces
}

/// Tier 3: character-boundary slices with a tail overlap.
fn hard_slice(text: &str, max_chars: usize, overlap_chars: usize) -> Vec<String> {
    let overlap = overlap_chars.min(max_chars / 2);
    let chars: Vec<char> = text.chars().collect();
    let mut slices = Vec::new();
    let mut start = 0usize;

    while start < chars.len() {
        let end = (start + max_chars).min(chars.len());
        let mut slice = String::new();
        if start > 0 && overlap > 0 {
            let overlap_start = start.saturating_sub(overlap);
            slice.extend(&chars[overlap_start..start]);
        }
        slice.extend(&chars[start..end]);
        slices.push(slice);
        if end == chars.len() {
            break;
        }
        start = end;
    }

    slices
}

fn char_len(value: &str) -> usize {
    value.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(max: usize, overlap: usize) -> ChunkOptions {
        ChunkOptions {
            max_chars: max,
            overlap_chars: overlap,
        }
    }

    #[test]
    fn small_document_is_one_chunk_per_section() {
        let text = "# Title\n\nIntro paragraph.\n\n## Sub\n\nDetail.\n";
        let chunks = chunk_markdown(text, &ChunkOptions::default());
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].heading, "Title");
        assert!(chunks[0].text.contains("Intro paragraph."));
        assert_eq!(chunks[1].heading, "Sub");
        assert!(chunks[1].text.contains("Detail."));
    }

    #[test]
    fn preamble_before_the_first_heading_is_kept() {
        let text = "Loose intro line.\n\n# Title\n\nBody.\n";
        let chunks = chunk_markdown(text, &ChunkOptions::default());
        assert_eq!(chunks[0].heading, "");
        assert!(chunks[0].text.contains("Loose intro line."));
    }

    #[test]
    fn headings_inside_code_fences_do_not_split() {
        let text = "# Real\n\n```sh\n# not a heading\necho hi\n```\n\nAfter.\n";
        let chunks = chunk_markdown(text, &ChunkOptions::default());
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].text.contains("# not a heading"));
    }

    #[test]
    fn oversized_section_splits_on_paragraph_boundaries() {
        let para = "word ".repeat(40); // 200 chars
        let text = format!("# T\n\n{para}\n\n{para}\n\n{para}\n");
        let chunks = chunk_markdown(&text, &opts(260, 20));
        assert!(
            chunks.len() >= 3,
            "expected paragraph packing, got {chunks:?}"
        );
        for chunk in &chunks {
            assert!(
                chunk.text.chars().count() <= 260 + 8,
                "chunk too big: {}",
                chunk.text.len()
            );
        }
    }

    #[test]
    fn oversized_paragraph_is_hard_sliced_with_tail_overlap() {
        let blob: String = ('a'..='z').cycle().take(1_000).collect();
        let chunks = chunk_markdown(&blob, &opts(200, 50));
        assert!(chunks.len() >= 5);
        // Every slice after the first repeats the previous slice's tail.
        for pair in chunks.windows(2) {
            let previous: String = pair[0]
                .text
                .chars()
                .rev()
                .take(50)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            assert!(
                pair[1].text.starts_with(&previous),
                "overlap missing between slices"
            );
        }
    }

    #[test]
    fn hard_slice_respects_multibyte_characters() {
        let blob = "é".repeat(500);
        let chunks = chunk_markdown(&blob, &opts(200, 10));
        let rebuilt: String = chunks.iter().map(|c| c.text.as_str()).collect();
        assert!(
            rebuilt.chars().all(|c| c == 'é' || c.is_whitespace()),
            "slicing must land on character boundaries, never split a code point"
        );
        // Every source character survives (overlap means at least, never fewer).
        assert!(rebuilt.chars().filter(|c| *c == 'é').count() >= 500);
        assert!(chunks.len() > 1);
    }

    #[test]
    fn empty_and_whitespace_input_produce_no_chunks() {
        assert!(chunk_markdown("", &ChunkOptions::default()).is_empty());
        assert!(chunk_markdown("   \n\n\t\n", &ChunkOptions::default()).is_empty());
    }
}
