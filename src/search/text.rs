//! Search index/query text helpers with exact source parity.
//!
//! Ports `packages/search/src/stem.ts`, `chosung.ts`, and
//! `normalizeSearchText` / index-field construction from `meili.ts` and
//! `packages/jobs/src/search-index.ts` at source SHA
//! `393795261322b916e588043cf94feca999175843`.
//!
//! Porter stemming is a line-by-line port of npm `stemmer@2.0.1`
//! (original Porter algorithm).

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;
use unicode_normalization::UnicodeNormalization;

// Porter Stemmer algorithm from npm `stemmer@2.0.1`.
//
// The MIT License
// Copyright (c) 2014 Titus Wormer <tituswormer@gmail.com>
//
// Permission is hereby granted, free of charge, to any person obtaining
// a copy of this software and associated documentation files (the
// "Software"), to deal in the Software without restriction, including
// without limitation the rights to use, copy, modify, merge, publish,
// distribute, sublicense, and/or sell copies of the Software, and to
// permit persons to whom the Software is furnished to do so, subject to
// the following conditions:
//
// The above copyright notice and this permission notice shall be
// included in all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
// EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
// MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
// IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
// CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT,
// TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE
// SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

const CONSONANT: &str = "[^aeiou]";
const VOWEL: &str = "[aeiouy]";

struct Porter {
    gt0: Regex,
    eq1: Regex,
    gt1: Regex,
    vowel_in_stem: Regex,
    consonant_like: Regex,
    sfx_ll: Regex,
    sfx_e: Regex,
    sfx_y: Regex,
    sfx_ion: Regex,
    sfx_ed_or_ing: Regex,
    sfx_at_or_bl_or_iz: Regex,
    sfx_eed: Regex,
    sfx_s: Regex,
    sfx_sses_or_ies: Regex,
    step2: Regex,
    step3: Regex,
    step4: Regex,
}

fn porter_re(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap_or_else(|err| panic!("porter regex {pattern}: {err}"))
}

impl Porter {
    fn new() -> Self {
        let consonants = format!("({CONSONANT}[^aeiouy]*)");
        let vowels = format!("({VOWEL}[aeiou]*)");
        Self {
            gt0: porter_re(&format!("^{consonants}?{vowels}{consonants}")),
            eq1: porter_re(&format!("^{consonants}?{vowels}{consonants}{vowels}?$")),
            gt1: porter_re(&format!("^{consonants}?({vowels}{consonants}){{2,}}")),
            vowel_in_stem: porter_re(&format!("^{consonants}?{VOWEL}")),
            consonant_like: porter_re(&format!("^{consonants}{VOWEL}[^aeiouwxy]$")),
            sfx_ll: porter_re("ll$"),
            sfx_e: porter_re("^(.+?)e$"),
            sfx_y: porter_re("^(.+?)y$"),
            sfx_ion: porter_re("^(.+?(s|t))(ion)$"),
            sfx_ed_or_ing: porter_re("^(.+?)(ed|ing)$"),
            sfx_at_or_bl_or_iz: porter_re("(at|bl|iz)$"),
            sfx_eed: porter_re("^(.+?)eed$"),
            sfx_s: porter_re("^.+?[^s]s$"),
            sfx_sses_or_ies: porter_re("^.+?(ss|i)es$"),
            step2: porter_re(
                "^(.+?)(ational|tional|enci|anci|izer|bli|alli|entli|eli|ousli|ization|ation|ator|alism|iveness|fulness|ousness|aliti|iviti|biliti|logi)$",
            ),
            step3: porter_re("^(.+?)(icate|ative|alize|iciti|ical|ful|ness)$"),
            step4: porter_re(
                "^(.+?)(al|ance|ence|er|ic|able|ible|ant|ement|ment|ent|ou|ism|ate|iti|ous|ive|ize)$",
            ),
        }
    }

    fn stem(&self, value: &str) -> String {
        let mut result = value.to_lowercase();
        if result.encode_utf16().count() < 3 {
            return result;
        }

        let mut first_character_was_lowercase_y = false;
        let mut chars = result.chars();
        if chars.next() == Some('y') {
            first_character_was_lowercase_y = true;
            result = format!("Y{}", chars.as_str());
        }

        if self.sfx_sses_or_ies.is_match(&result) {
            result.truncate(result.len().saturating_sub(2));
        } else if self.sfx_s.is_match(&result) {
            result.truncate(result.len().saturating_sub(1));
        }

        if let Some(caps) = self.sfx_eed.captures(&result) {
            if self.gt0.is_match(&caps[1]) {
                result.truncate(result.len().saturating_sub(1));
            }
        } else if let Some(caps) = self.sfx_ed_or_ing.captures(&result) {
            if self.vowel_in_stem.is_match(&caps[1]) {
                result = caps[1].to_string();
                if self.sfx_at_or_bl_or_iz.is_match(&result) {
                    result.push('e');
                } else if sfx_multi_consonant_like(&result) {
                    result.truncate(result.len().saturating_sub(1));
                } else if self.consonant_like.is_match(&result) {
                    result.push('e');
                }
            }
        }

        if let Some(caps) = self.sfx_y.captures(&result) {
            if self.vowel_in_stem.is_match(&caps[1]) {
                result = format!("{}i", &caps[1]);
            }
        }

        if let Some(caps) = self.step2.captures(&result) {
            if self.gt0.is_match(&caps[1]) {
                result = format!("{}{}", &caps[1], step2_replacement(&caps[2]));
            }
        }

        if let Some(caps) = self.step3.captures(&result) {
            if self.gt0.is_match(&caps[1]) {
                result = format!("{}{}", &caps[1], step3_replacement(&caps[2]));
            }
        }

        if let Some(caps) = self.step4.captures(&result) {
            if self.gt1.is_match(&caps[1]) {
                result = caps[1].to_string();
            }
        } else if let Some(caps) = self.sfx_ion.captures(&result) {
            if self.gt1.is_match(&caps[1]) {
                result = caps[1].to_string();
            }
        }

        if let Some(caps) = self.sfx_e.captures(&result) {
            let stem = &caps[1];
            if self.gt1.is_match(stem)
                || (self.eq1.is_match(stem) && !self.consonant_like.is_match(stem))
            {
                result = stem.to_string();
            }
        }

        if self.sfx_ll.is_match(&result) && self.gt1.is_match(&result) {
            result.truncate(result.len().saturating_sub(1));
        }

        if first_character_was_lowercase_y {
            let mut chars = result.chars();
            chars.next();
            result = format!("y{}", chars.as_str());
        }

        result
    }
}

/// npm `stemmer` `/([^aeiouylsz])\1$/` (rust `regex` has no backreferences).
fn sfx_multi_consonant_like(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() < 2 {
        return false;
    }
    let last = b[b.len() - 1];
    last == b[b.len() - 2]
        && !matches!(
            last,
            b'a' | b'e' | b'i' | b'o' | b'u' | b'y' | b'l' | b's' | b'z'
        )
}

fn step2_replacement(suffix: &str) -> &'static str {
    match suffix {
        "ational" => "ate",
        "tional" => "tion",
        "enci" => "ence",
        "anci" => "ance",
        "izer" => "ize",
        "bli" => "ble",
        "alli" => "al",
        "entli" => "ent",
        "eli" => "e",
        "ousli" => "ous",
        "ization" => "ize",
        "ation" => "ate",
        "ator" => "ate",
        "alism" => "al",
        "iveness" => "ive",
        "fulness" => "ful",
        "ousness" => "ous",
        "aliti" => "al",
        "iviti" => "ive",
        "biliti" => "ble",
        "logi" => "log",
        other => panic!("unexpected step2 suffix {other}"),
    }
}

fn step3_replacement(suffix: &str) -> &'static str {
    match suffix {
        "icate" => "ic",
        "ative" => "",
        "alize" => "al",
        "iciti" => "ic",
        "ical" => "ic",
        "ful" => "",
        "ness" => "",
        other => panic!("unexpected step3 suffix {other}"),
    }
}

static PORTER: LazyLock<Porter> = LazyLock::new(Porter::new);

/// Original Porter stem for `value`, matching npm `stemmer@2.0.1`.
pub fn porter_stem(value: &str) -> String {
    // The Porter port works on ASCII bytes; stem_text only passes [a-z]+ tokens.
    if !value.is_ascii() {
        return value.to_string();
    }
    PORTER.stem(value)
}

// Source: PostgreSQL english.stop (tsearch_data/english.stop), `stem.ts`.
const ENGLISH_STOP_WORDS: &[&str] = &[
    "i",
    "me",
    "my",
    "myself",
    "we",
    "our",
    "ours",
    "ourselves",
    "you",
    "your",
    "yours",
    "yourself",
    "yourselves",
    "he",
    "him",
    "his",
    "himself",
    "she",
    "her",
    "hers",
    "herself",
    "it",
    "its",
    "itself",
    "they",
    "them",
    "their",
    "theirs",
    "themselves",
    "what",
    "which",
    "who",
    "whom",
    "this",
    "that",
    "these",
    "those",
    "am",
    "is",
    "are",
    "was",
    "were",
    "be",
    "been",
    "being",
    "have",
    "has",
    "had",
    "having",
    "do",
    "does",
    "did",
    "doing",
    "a",
    "an",
    "the",
    "and",
    "but",
    "if",
    "or",
    "because",
    "as",
    "until",
    "while",
    "of",
    "at",
    "by",
    "for",
    "with",
    "about",
    "against",
    "between",
    "into",
    "through",
    "during",
    "before",
    "after",
    "above",
    "below",
    "to",
    "from",
    "up",
    "down",
    "in",
    "out",
    "on",
    "off",
    "over",
    "under",
    "again",
    "further",
    "then",
    "once",
    "here",
    "there",
    "when",
    "where",
    "why",
    "how",
    "all",
    "any",
    "both",
    "each",
    "few",
    "more",
    "most",
    "other",
    "some",
    "such",
    "no",
    "nor",
    "not",
    "only",
    "own",
    "same",
    "so",
    "than",
    "too",
    "very",
    "s",
    "t",
    "can",
    "will",
    "just",
    "don",
    "should",
    "now",
];

static ENGLISH_STOP_WORD_SET: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| ENGLISH_STOP_WORDS.iter().copied().collect());

static TOKEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\p{L}\p{N}]+").expect("stem token regex"));

fn is_ascii_alpha(token: &str) -> bool {
    !token.is_empty() && token.bytes().all(|b| b.is_ascii_lowercase())
}

/// Meili `stem` field builder: NFKC, lowercase, `[\p{L}\p{N}]+` tokens,
/// English stop-word drop + Porter on `[a-z]+` only, first-seen order.
pub fn stem_text(text: &str) -> String {
    let lowered: String = text.nfkc().collect::<String>().to_lowercase();
    let mut seen = HashSet::new();
    let mut ordered = Vec::new();
    for mat in TOKEN_RE.find_iter(&lowered) {
        let token = mat.as_str();
        let piece = if is_ascii_alpha(token) {
            if ENGLISH_STOP_WORD_SET.contains(token) {
                continue;
            }
            porter_stem(token)
        } else {
            token.to_string()
        };
        if seen.insert(piece.clone()) {
            ordered.push(piece);
        }
    }
    ordered.join(" ")
}

const CHOSUNG: [&str; 19] = [
    "ㄱ", "ㄲ", "ㄴ", "ㄷ", "ㄸ", "ㄹ", "ㅁ", "ㅂ", "ㅃ", "ㅅ", "ㅆ", "ㅇ", "ㅈ", "ㅉ", "ㅊ", "ㅋ",
    "ㅌ", "ㅍ", "ㅎ",
];
const HANGUL_FIRST: u32 = 0xAC00;
const HANGUL_LAST: u32 = 0xD7A3;
const SYLLABLES_PER_CHOSUNG: u32 = 21 * 28;

/// ECMAScript `\s` (no `u` flag): Unicode White_Space plus U+FEFF, minus NEL.
fn is_ecmascript_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{0009}'
            | '\u{000A}'
            | '\u{000B}'
            | '\u{000C}'
            | '\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

fn is_compat_choseong(c: char) -> bool {
    ('ㄱ'..='ㅎ').contains(&c)
}

/// Source `isChosungQuery`: compatibility jamo `ㄱ-ㅎ` only, ES `\s` allowed.
pub fn is_chosung_query(q: &str) -> bool {
    let mut has_jamo = false;
    for c in q.chars() {
        if is_compat_choseong(c) {
            has_jamo = true;
        } else if !is_ecmascript_whitespace(c) {
            return false;
        }
    }
    has_jamo
}

/// Source `toChosung`: Hangul syllables → compatibility choseong; other chars kept.
pub fn to_chosung(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        let cp = ch as u32;
        if (HANGUL_FIRST..=HANGUL_LAST).contains(&cp) {
            let idx = ((cp - HANGUL_FIRST) / SYLLABLES_PER_CHOSUNG) as usize;
            if let Some(jamo) = CHOSUNG.get(idx) {
                out.push_str(jamo);
                continue;
            }
        }
        out.push(ch);
    }
    out
}

/// Source `normalizeSearchText`: Unicode NFKC. Not applied to chosung queries.
pub fn normalize_search_text(s: &str) -> String {
    s.nfkc().collect()
}

/// Query stem passed to Meili: empty when the (already trimmed) query is chosung-only.
pub fn query_stem(q: &str) -> String {
    if is_chosung_query(q) {
        String::new()
    } else {
        stem_text(q)
    }
}

/// Title/body/stem/chosung fields for a Meili document, matching `toMeili`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedText {
    pub title: String,
    pub body: String,
    pub stem: String,
    pub chosung: String,
}

/// NFKC title/body, `stemText(title + " " + body)`, stored chosung or `toChosung`.
pub fn index_document_text(title: &str, body: &str, stored_chosung: &str) -> IndexedText {
    let title = normalize_search_text(title);
    let body = normalize_search_text(body);
    let joined = format!("{title} {body}");
    let stored = stored_chosung.trim();
    let chosung = if stored.is_empty() {
        to_chosung(&joined)
    } else {
        stored.to_string()
    };
    IndexedText {
        stem: stem_text(&joined),
        chosung,
        title,
        body,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::fs;
    use std::path::PathBuf;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/search")
            .join(name)
    }

    #[derive(Deserialize)]
    struct StemCase {
        #[serde(rename = "in")]
        input: String,
        out: String,
    }

    #[derive(Deserialize)]
    struct ChosungCase {
        #[serde(rename = "in")]
        input: String,
        chosung: String,
        #[serde(rename = "isChosung")]
        is_chosung: bool,
    }

    #[derive(Deserialize)]
    struct NormalizeCase {
        #[serde(rename = "in")]
        input: String,
        out: String,
    }

    fn assert_bytes(got: &str, expected: &str, label: &str) {
        assert_eq!(
            got.as_bytes(),
            expected.as_bytes(),
            "{label}: got {got:?} expected {expected:?}"
        );
    }

    #[test]
    fn porter_and_stem_text_match_enable_oracle() {
        let raw = fs::read_to_string(fixture("porter_stems.tsv")).expect("porter_stems.tsv");
        let mut n = 0usize;
        let mut mismatches = Vec::new();
        for (line_no, line) in raw.lines().enumerate() {
            if line.is_empty() {
                continue;
            }
            let mut parts = line.split('\t');
            let word = parts.next().expect("word");
            let stem = parts.next().expect("porter");
            let stem_text_expected = parts.next().expect("stemText");
            n += 1;
            let got = porter_stem(word);
            if got.as_bytes() != stem.as_bytes() {
                mismatches.push(format!(
                    "L{} porter {word:?}: got {got:?} expected {stem:?}",
                    line_no + 1
                ));
            }
            let got_st = stem_text(word);
            if got_st.as_bytes() != stem_text_expected.as_bytes() {
                mismatches.push(format!(
                    "L{} stem_text {word:?}: got {got_st:?} expected {stem_text_expected:?}",
                    line_no + 1
                ));
            }
            if mismatches.len() >= 24 {
                break;
            }
        }
        assert!(
            mismatches.is_empty(),
            "{} mismatches (showing up to 24): {mismatches:?}",
            mismatches.len()
        );
        assert!(n >= 20_000, "ENABLE oracle too small: {n}");
        assert_eq!(n, 172_823, "ENABLE oracle count");
    }

    #[test]
    fn stem_text_matches_mixed_oracle() {
        let raw = fs::read_to_string(fixture("stem_text.json")).expect("stem_text.json");
        let cases: Vec<StemCase> = serde_json::from_str(&raw).expect("stem_text json");
        assert_eq!(cases.len(), 74);
        for case in &cases {
            assert_bytes(&stem_text(&case.input), &case.out, &case.input);
        }
    }

    #[test]
    fn chosung_matches_oracle() {
        let raw = fs::read_to_string(fixture("chosung.json")).expect("chosung.json");
        let cases: Vec<ChosungCase> = serde_json::from_str(&raw).expect("chosung json");
        assert_eq!(cases.len(), 44);
        for case in &cases {
            assert_bytes(&to_chosung(&case.input), &case.chosung, &case.input);
            assert_eq!(
                is_chosung_query(&case.input),
                case.is_chosung,
                "is_chosung_query {:?}",
                case.input
            );
        }
    }

    #[test]
    fn normalize_matches_oracle() {
        let raw = fs::read_to_string(fixture("normalize.json")).expect("normalize.json");
        let cases: Vec<NormalizeCase> = serde_json::from_str(&raw).expect("normalize json");
        assert_eq!(cases.len(), 22);
        for case in &cases {
            assert_bytes(&normalize_search_text(&case.input), &case.out, &case.input);
        }
    }

    #[test]
    fn source_unit_examples() {
        assert_bytes(
            &stem_text("Running the Search Engine indexed documents; runs the index"),
            "run search engin index document",
            "english sentence",
        );
        assert_bytes(&stem_text("검색 running 2024"), "검색 run 2024", "mixed");
        assert_bytes(&stem_text("the of and"), "", "stop only");
        assert_bytes(&stem_text("Ｒunning"), "run", "fullwidth");
        assert!(is_chosung_query("ㄱㅅ"));
        assert!(is_chosung_query("ㄱㅅ ㅇㅈ"));
        assert!(!is_chosung_query("검색"));
        assert!(!is_chosung_query(""));
        assert_bytes(&to_chosung("한글"), "ㅎㄱ", "hangul");
        assert_bytes(&to_chosung("FVOCI 검색"), "FVOCI ㄱㅅ", "mixed chosung");
        assert_bytes(&normalize_search_text("ＦＶＯＣＩ"), "FVOCI", "nfkc");
        assert_eq!(query_stem("ㄱㅅ"), "");
        assert_eq!(query_stem("running"), "run");
        let indexed = index_document_text("Ｒunning", "the Search", "");
        assert_eq!(indexed.title, "Running");
        assert_eq!(indexed.body, "the Search");
        assert_eq!(indexed.stem, "run search");
        assert_eq!(indexed.chosung, "Running the Search");
        let stored = index_document_text("검색", "엔진", "  ㄱ  ");
        assert_eq!(stored.chosung, "ㄱ");
        assert_eq!(stored.stem, "검색 엔진");
    }
}
