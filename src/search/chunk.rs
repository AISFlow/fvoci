//! UTF-16 `chunkPlainText` from `packages/contracts/src/chunk.ts`.
//!
//! Offsets are JS `String.length` units so web highlight and Meili chunk ids match.

const TARGET: usize = 1500;
const MIN: usize = 800;
const MAX: usize = 2000;
const OVERLAP: usize = 150;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextChunk {
    pub chunk_no: i32,
    pub start: i32,
    pub end: i32,
    pub text: String,
}

fn is_js_space(unit: u16) -> bool {
    matches!(
        unit,
        0x09 | 0x0a
            | 0x0b
            | 0x0c
            | 0x0d
            | 0x20
            | 0xa0
            | 0x1680
            | 0x2028
            | 0x2029
            | 0x202f
            | 0x205f
            | 0x3000
            | 0xfeff
    ) || (0x2000..=0x200a).contains(&unit)
}

/// WHY: boundary = first unit after a page break or blank line, swallowing following whitespace.
fn boundaries(units: &[u16]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < units.len() {
        if units[i] == 0x0c {
            let mut j = i + 1;
            while j < units.len() && is_js_space(units[j]) {
                j += 1;
            }
            out.push(j);
            i = j.max(i + 1);
            continue;
        }
        if units[i] == 0x0a {
            let mut j = i + 1;
            while j < units.len() && (units[j] == 0x20 || units[j] == 0x09) {
                j += 1;
            }
            if j < units.len() && units[j] == 0x0a {
                j += 1;
                while j < units.len() && is_js_space(units[j]) {
                    j += 1;
                }
                out.push(j);
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out
}

pub fn chunk_plain_text(text: &str) -> Vec<TextChunk> {
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.is_empty() {
        return Vec::new();
    }
    let bounds = boundaries(&units);
    let mut chunks = Vec::new();
    let mut start = 0usize;
    while start < units.len() {
        let mut end = units.len();
        if end - start > MAX {
            let goal = start + TARGET;
            let near: Vec<usize> = bounds
                .iter()
                .copied()
                .filter(|b| *b >= start + MIN && *b <= start + MAX)
                .collect();
            end = if near.is_empty() {
                goal.min(units.len())
            } else {
                near.into_iter()
                    .min_by_key(|b| b.abs_diff(goal))
                    .unwrap_or(goal)
            };
        }
        chunks.push(TextChunk {
            chunk_no: chunks.len() as i32,
            start: start as i32,
            end: end as i32,
            text: String::from_utf16_lossy(&units[start..end]),
        });
        if end >= units.len() {
            break;
        }
        start = end.saturating_sub(OVERLAP);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_empty() {
        assert!(chunk_plain_text("").is_empty());
    }

    #[test]
    fn short_text_is_one_chunk() {
        let chunks = chunk_plain_text("hello");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].chunk_no, 0);
        assert_eq!(chunks[0].start, 0);
        assert_eq!(chunks[0].end, 5);
        assert_eq!(chunks[0].text, "hello");
    }

    #[test]
    fn utf16_length_counts_surrogate_pairs() {
        let text = "a😀b";
        assert_eq!(text.encode_utf16().count(), 4);
        let chunks = chunk_plain_text(text);
        assert_eq!(chunks[0].end, 4);
        assert_eq!(chunks[0].text, text);
    }

    #[test]
    fn blank_line_is_preferred_near_target() {
        let first = "x".repeat(900);
        let second = "y".repeat(1200);
        let text = format!("{first}\n\n{second}");
        assert!(text.encode_utf16().count() > MAX);
        let chunks = chunk_plain_text(&text);
        assert!(chunks.len() >= 2);
        assert_eq!(chunks[0].end as usize, 902);
        assert_eq!(chunks[0].text, format!("{first}\n\n"));
        assert!(chunks[1].text.contains('y'));
    }

    #[test]
    fn overlap_is_150_utf16_units() {
        let text = "z".repeat(3500);
        let chunks = chunk_plain_text(&text);
        assert!(chunks.len() >= 2);
        let overlap = chunks[0].end - chunks[1].start;
        assert_eq!(overlap, OVERLAP as i32);
    }
}
