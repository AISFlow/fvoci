//! The piece of a body a search in the database shows: about [`SNIPPET_CHARS`]
//! characters around the first place a word asked for is, in stretches, every place
//! of every word in it marked as matched. Words are found as the database finds them:
//! by their letters, ASCII letters in either case.

use std::ops::Range;

use fvoci_model::SnippetPart;

/// About how many characters a snippet shows, as many as a search server shows.
const SNIPPET_CHARS: usize = 160;
/// How many characters of the text before the first match a snippet shows, at most.
const BEFORE_CHARS: usize = 40;

/// The snippet of `text` for `words`: the stretches of about [`SNIPPET_CHARS`]
/// characters from a little before the first word found, or from the start where none
/// is (the words were in the title). Empty for an empty text.
pub(super) fn snippet(text: &str, words: &[String]) -> Vec<SnippetPart> {
    // ASCII letters in lower case keep every byte where it is, and only those letters
    // change: a place found in the folded text is the same place in the text, on the
    // boundaries of its characters.
    let folded = text.to_ascii_lowercase();
    let words: Vec<String> = words
        .iter()
        .filter(|word| !word.is_empty())
        .map(|word| word.to_ascii_lowercase())
        .collect();
    let first = words
        .iter()
        .filter_map(|word| folded.find(word.as_str()).map(|at| at..at + word.len()))
        .min_by_key(|place| place.start);
    let start = back_by_chars(
        text,
        first.as_ref().map_or(0, |found| found.start),
        BEFORE_CHARS,
    );
    let end = forward_by_chars(text, start, SNIPPET_CHARS).max(
        // A word longer than the snippet is shown whole.
        first.map_or(0, |found| found.end),
    );
    let mut parts: Vec<SnippetPart> = Vec::new();
    let mut at = start;
    for found in places(&folded, &words, start..end) {
        let shown = found.start.max(at)..found.end.min(end);
        if shown.is_empty() {
            continue;
        }
        push(&mut parts, &text[at..shown.start], Stretch::Plain);
        push(&mut parts, &text[shown.clone()], Stretch::Matched);
        at = shown.end;
    }
    push(&mut parts, &text[at..end], Stretch::Plain);
    parts
}

/// Whether a stretch matched a word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stretch {
    Plain,
    Matched,
}

/// Adds a stretch, unless it is empty.
fn push(parts: &mut Vec<SnippetPart>, text: &str, stretch: Stretch) {
    if !text.is_empty() {
        parts.push(SnippetPart {
            text: text.to_owned(),
            matched: stretch == Stretch::Matched,
        });
    }
}

/// Every place of every word of `words` in `folded` that begins within `within`, in
/// byte offsets, in order, the places that overlap or touch one another made one. The
/// text past `within` is not looked through.
fn places(folded: &str, words: &[String], within: Range<usize>) -> Vec<Range<usize>> {
    let mut places: Vec<Range<usize>> = Vec::new();
    for word in words {
        places.extend(
            folded[within.start..]
                .match_indices(word.as_str())
                .map(|(at, found)| within.start + at..within.start + at + found.len())
                .take_while(|place| place.start < within.end),
        );
    }
    places.sort_unstable_by_key(|place| (place.start, place.end));
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(places.len());
    for place in places {
        match merged.last_mut() {
            Some(last) if place.start <= last.end => last.end = last.end.max(place.end),
            _ => merged.push(place),
        }
    }
    merged
}

/// The byte offset `chars` characters before `at` in `text`, or its start.
fn back_by_chars(text: &str, at: usize, chars: usize) -> usize {
    text[..at]
        .char_indices()
        .rev()
        .nth(chars.saturating_sub(1))
        .map_or(0, |(offset, _)| offset)
}

/// The byte offset `chars` characters after `at` in `text`, or its end.
fn forward_by_chars(text: &str, at: usize, chars: usize) -> usize {
    text[at..]
        .char_indices()
        .nth(chars)
        .map_or(text.len(), |(offset, _)| at + offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(words: &[&str]) -> Vec<String> {
        words.iter().map(|&word| word.to_owned()).collect()
    }

    fn shown(parts: &[SnippetPart]) -> Vec<(&str, bool)> {
        parts
            .iter()
            .map(|part| (part.text.as_str(), part.matched))
            .collect()
    }

    #[test]
    fn every_place_of_every_word_is_marked() {
        let parts = snippet(
            "결제 후 환불규정을 따라 환불한다",
            &words(&["환불", "따라"]),
        );
        assert_eq!(
            shown(&parts),
            [
                ("결제 후 ", false),
                ("환불", true),
                ("규정을 ", false),
                ("따라", true),
                (" ", false),
                ("환불", true),
                ("한다", false),
            ]
        );
    }

    #[test]
    fn ascii_letters_match_in_either_case_and_no_other_letter_does() {
        let parts = snippet("The API and the api. Été", &words(&["api", "été"]));
        assert_eq!(
            shown(&parts),
            [
                ("The ", false),
                ("API", true),
                (" and the ", false),
                ("api", true),
                (". Été", false),
            ]
        );
    }

    #[test]
    fn words_that_overlap_are_one_stretch() {
        let parts = snippet("환불규정", &words(&["환불규", "불규정"]));
        assert_eq!(shown(&parts), [("환불규정", true)]);
    }

    #[test]
    fn a_long_body_is_shown_around_its_first_match_on_character_boundaries() {
        let text = format!("{}환불{}", "가".repeat(500), "😀".repeat(500));
        let parts = snippet(&text, &words(&["환불"]));
        let all: String = parts.iter().map(|part| part.text.as_str()).collect();
        assert_eq!(all.chars().count(), SNIPPET_CHARS);
        assert_eq!(parts[0].text, "가".repeat(BEFORE_CHARS));
        assert_eq!(shown(&parts)[1], ("환불", true));
        assert_eq!(parts[2].text, "😀".repeat(SNIPPET_CHARS - BEFORE_CHARS - 2));
    }

    #[test]
    fn a_match_at_either_end_and_none_at_all() {
        assert_eq!(shown(&snippet("환불", &words(&["환불"]))), [("환불", true)]);
        assert_eq!(
            shown(&snippet("규정 환불", &words(&["환불"]))),
            [("규정 ", false), ("환불", true)]
        );
        let text = "😀".repeat(200);
        let parts = snippet(&text, &words(&["제목에만"]));
        assert_eq!(parts.len(), 1);
        assert!(!parts[0].matched);
        assert_eq!(parts[0].text.chars().count(), SNIPPET_CHARS);
        assert_eq!(snippet("", &words(&["환불"])), []);
    }

    #[test]
    fn a_word_longer_than_the_snippet_is_shown_whole() {
        let long = "나".repeat(SNIPPET_CHARS + 10);
        let text = format!("{}{long}", "가".repeat(100));
        let parts = snippet(&text, &words(&[&long]));
        assert_eq!(shown(&parts)[1], (long.as_str(), true));
        assert_eq!(parts.len(), 2);
    }
}
