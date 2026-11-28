//! Reactions: who answered a comment, a card, a page or a unit with which emoji.
//!
//! A reaction is no change of the block: it is kept beside it, and neither makes the
//! block "changed" (`updatedAt`) nor tells anybody of itself but those who read the
//! block. One emoji is any one emoji the server takes the shape of ([`Emoji`]); a block
//! has at most [`REACTION_KINDS_PER_BLOCK`] different ones.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use unicode_segmentation::UnicodeSegmentation as _;

/// How many different emoji one block may be answered with. Who adds one of them
/// again, or adds their own to one that is there, is not counted against it.
pub const REACTION_KINDS_PER_BLOCK: usize = 20;

/// The most Unicode scalar values an emoji may have: a person with a skin tone and a
/// role joined to it is at most this long, as is every emoji the web app offers.
pub const EMOJI_MAX_SCALARS: usize = 8;

/// One emoji, as a reaction names it: one grapheme of at most [`EMOJI_MAX_SCALARS`]
/// scalar values that has the shape of an emoji. Only [`FromStr`] makes one.
///
/// The server takes the shape alone, not a list: a pictograph with the selector of the
/// emoji's look and its skin tone, joined to others by zero-width joiners (`👩🏽‍💻`), a
/// flag of two regional indicators (`🇰🇷`) or of a region's 3 to 6 tags (`🏴󠁧󠁢󠁳󠁣󠁴󠁿`), or a
/// keycap (`#️⃣`). Letters, digits, spaces and control characters on their own are none,
/// and so is the text's look of an emoji.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct Emoji(String);

impl Emoji {
    /// The emoji's text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Emoji {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for Emoji {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Why a text is no [`Emoji`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ParseEmojiError {
    /// The text is empty.
    #[error("an emoji is not empty")]
    Empty,
    /// The text has more scalar values than an emoji may.
    #[error("an emoji is at most {EMOJI_MAX_SCALARS} characters")]
    TooLong,
    /// The text is more than one grapheme, or has not the shape of an emoji.
    #[error("a reaction is one emoji")]
    NotAnEmoji,
}

impl FromStr for Emoji {
    type Err = ParseEmojiError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let scalars: Vec<char> = text.chars().collect();
        if scalars.is_empty() {
            return Err(ParseEmojiError::Empty);
        }
        if scalars.len() > EMOJI_MAX_SCALARS {
            return Err(ParseEmojiError::TooLong);
        }
        if text.graphemes(true).count() != 1 || !has_the_shape_of_an_emoji(&scalars) {
            return Err(ParseEmojiError::NotAnEmoji);
        }
        Ok(Self(text.to_owned()))
    }
}

impl<'de> Deserialize<'de> for Emoji {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// Who answered a block with one emoji.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Reaction {
    /// The emoji.
    #[schema(value_type = String)]
    pub emoji: Emoji,
    /// Who answered with it, the first of them first; never empty.
    pub user_ids: Vec<String>,
}

/// Defines a read of a block of one kind with the reactions to it: what the members of
/// its container read, and nobody else (a public link's reader reads no user identifier,
/// which the reactions are).
macro_rules! with_reactions {
    ($(#[$doc:meta])* $name:ident, $member:ident: $read:ty) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
        #[serde(rename_all = "camelCase")]
        pub struct $name {
            /// What is read, its members beside `reactions`.
            #[serde(flatten)]
            pub $member: $read,
            /// The reactions to it, the emoji it was first answered with first; left out
            /// where there are none.
            #[serde(default, skip_serializing_if = "Vec::is_empty")]
            pub reactions: Vec<Reaction>,
        }
    };
}

with_reactions!(
    /// A comment as the comments of a card, a page or a unit list it: the block, and the
    /// reactions to it.
    CommentWithReactions,
    comment: crate::Block
);
with_reactions!(
    /// A card as `getCard` reads it, and the reactions to it.
    CardWithReactions,
    card: crate::Card
);
with_reactions!(
    /// A page as `getPage` reads it, and the reactions to it.
    PageWithReactions,
    page: crate::Page
);
with_reactions!(
    /// A unit or a chapter as `getUnit` reads it, and the reactions to it.
    UnitWithReactions,
    unit: crate::Unit
);

/// The zero-width joiner, between the parts of one emoji.
const JOINER: char = '\u{200D}';
/// The variation selector that asks for the emoji's look.
const EMOJI_LOOK: char = '\u{FE0F}';
/// The mark that makes a digit, `#` or `*` a keycap.
const KEYCAP: char = '\u{20E3}';
/// The black flag that tags make the flag of a region (`🏴󠁧󠁢󠁳󠁣󠁴󠁿`).
const TAGGED_FLAG: char = '\u{1F3F4}';
/// The tag that ends the tags of a flag.
const CANCEL_TAG: char = '\u{E007F}';

/// Whether the scalars are emoji parts joined by zero-width joiners, each part one
/// [`part_length`] takes whole.
fn has_the_shape_of_an_emoji(scalars: &[char]) -> bool {
    let mut rest = scalars;
    loop {
        let Some(taken) = part_length(rest) else {
            return false;
        };
        rest = &rest[taken..];
        match rest {
            [] => return true,
            [JOINER, after @ ..] if !after.is_empty() => rest = after,
            _ => return false,
        }
    }
}

/// How many scalars at the start of `scalars` make one part of an emoji, `None` if
/// they make none: a flag of two regional indicators; a keycap; or a pictograph (or a
/// skin tone alone) with the selector of the emoji's look, its skin tone and, on the
/// black flag, the tags of a region. The selector of the text's look (U+FE0E) is no
/// part: what it asks for is no emoji, and the same pictograph would be answered
/// with twice, once in each look.
fn part_length(scalars: &[char]) -> Option<usize> {
    match scalars {
        [first, second, ..] if is_regional_indicator(*first) && is_regional_indicator(*second) => {
            Some(2)
        }
        [key, EMOJI_LOOK, KEYCAP, ..] if is_keycap_base(*key) => Some(3),
        [key, KEYCAP, ..] if is_keycap_base(*key) => Some(2),
        [base, ..] if is_pictograph(*base) || is_skin_tone(*base) => {
            let mut taken = 1;
            if scalars.get(taken) == Some(&EMOJI_LOOK) {
                taken += 1;
            }
            if scalars.get(taken).copied().is_some_and(is_skin_tone) && !is_skin_tone(*base) {
                taken += 1;
                if scalars.get(taken) == Some(&EMOJI_LOOK) {
                    taken += 1;
                }
            }
            if *base == TAGGED_FLAG && scalars.get(taken).copied().is_some_and(is_tag) {
                let tags = scalars[taken..]
                    .iter()
                    .take_while(|&&scalar| is_tag(scalar))
                    .count();
                if !REGION_TAGS.contains(&tags) || scalars.get(taken + tags) != Some(&CANCEL_TAG) {
                    return None;
                }
                taken += tags + 1;
            }
            Some(taken)
        }
        _ => None,
    }
}

fn is_regional_indicator(scalar: char) -> bool {
    ('\u{1F1E6}'..='\u{1F1FF}').contains(&scalar)
}

fn is_skin_tone(scalar: char) -> bool {
    ('\u{1F3FB}'..='\u{1F3FF}').contains(&scalar)
}

fn is_keycap_base(scalar: char) -> bool {
    scalar.is_ascii_digit() || scalar == '#' || scalar == '*'
}

/// How many tags spell a region after the black flag: a subdivision's code of ISO
/// 3166-2 without its hyphen (`gbsct` for Scotland), from 3 to 6 letters and digits.
const REGION_TAGS: std::ops::RangeInclusive<usize> = 3..=6;

/// The tags that spell a region after the black flag, without the one that ends them:
/// the tag forms of the small letters and the digits alone, so that no other text
/// rides unseen in a flag.
fn is_tag(scalar: char) -> bool {
    matches!(scalar, '\u{E0030}'..='\u{E0039}' | '\u{E0061}'..='\u{E007A}')
}

/// Whether the scalar is a pictograph an emoji is made of: the blocks of Unicode's
/// emoji (`Extended_Pictographic`, a little wider), without the regional indicators
/// and the skin tones, which are parts of their own, and without the circled digits
/// and the arrow among the dingbats (`❶` to `➔`), which are no emoji.
fn is_pictograph(scalar: char) -> bool {
    const SINGLES: [char; 16] = [
        '\u{00A9}', '\u{00AE}', '\u{203C}', '\u{2049}', '\u{2122}', '\u{2139}', '\u{2328}',
        '\u{2388}', '\u{23CF}', '\u{24C2}', '\u{25B6}', '\u{25C0}', '\u{3030}', '\u{303D}',
        '\u{3297}', '\u{3299}',
    ];
    const RANGES: [(char, char); 17] = [
        ('\u{2194}', '\u{2199}'),
        ('\u{21A9}', '\u{21AA}'),
        ('\u{231A}', '\u{231B}'),
        ('\u{23E9}', '\u{23F3}'),
        ('\u{23F8}', '\u{23FA}'),
        ('\u{25AA}', '\u{25AB}'),
        ('\u{25FB}', '\u{25FE}'),
        ('\u{2600}', '\u{2775}'),
        ('\u{2795}', '\u{27BF}'),
        ('\u{2934}', '\u{2935}'),
        ('\u{2B05}', '\u{2B07}'),
        ('\u{2B1B}', '\u{2B1C}'),
        ('\u{2B50}', '\u{2B50}'),
        ('\u{2B55}', '\u{2B55}'),
        ('\u{1F000}', '\u{1F1E5}'),
        ('\u{1F200}', '\u{1FAFF}'),
        ('\u{1FC00}', '\u{1FFFD}'),
    ];
    !is_skin_tone(scalar)
        && (SINGLES.contains(&scalar)
            || RANGES
                .iter()
                .any(|&(first, last)| (first..=last).contains(&scalar)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The emoji of the web app's table (`apps/web/src/lib/editor/emoji/en.json`),
    /// which its picker offers: every one is taken.
    #[test]
    fn every_emoji_the_web_app_offers_is_taken() {
        let table: serde_json::Value = serde_json::from_str(include_str!(
            "../../../apps/web/src/lib/editor/emoji/en.json"
        ))
        .expect("the table is JSON");
        let emoji = table["emoji"].as_str().expect("the table lists its emoji");
        let refused: Vec<(&str, ParseEmojiError)> = emoji
            .split(' ')
            .filter_map(|one| one.parse::<Emoji>().err().map(|error| (one, error)))
            .collect();
        assert!(emoji.split(' ').count() > 1000, "the whole table is read");
        assert_eq!(refused, [], "every emoji of the table is one");
    }

    #[test]
    fn the_shapes_of_emoji_are_taken() {
        for one in [
            "👍",
            "❤️",
            "🎉",
            "👀",
            "🚀",
            "😄",
            "👍🏽",
            "👩🏽‍💻",
            "🏳️‍🌈",
            "❤️‍🔥",
            "👁️‍🗨️",
            "🧑🏻‍🦰",
            "👨‍👩‍👧‍👦",
            "🇰🇷",
            "🏴󠁧󠁢󠁳󠁣󠁴󠁿",
            "#️⃣",
            "1️⃣",
            "©️",
            "™",
            "🀄",
            "🫠",
            "🏃‍➡️",
            "🏽",
            "➕",
            "➗",
            "➡️",
            "➰",
        ] {
            assert_eq!(
                one.parse::<Emoji>().map(|emoji| emoji.to_string()),
                Ok(one.to_owned()),
                "{one:?}"
            );
        }
    }

    #[test]
    fn what_is_no_emoji_is_refused() {
        for (text, error) in [
            ("", ParseEmojiError::Empty),
            ("a", ParseEmojiError::NotAnEmoji),
            ("1", ParseEmojiError::NotAnEmoji),
            ("#", ParseEmojiError::NotAnEmoji),
            (" ", ParseEmojiError::NotAnEmoji),
            ("\n", ParseEmojiError::NotAnEmoji),
            ("\u{0007}", ParseEmojiError::NotAnEmoji),
            ("가", ParseEmojiError::NotAnEmoji),
            ("ok", ParseEmojiError::NotAnEmoji),
            ("👍👍", ParseEmojiError::NotAnEmoji),
            ("👍 ", ParseEmojiError::NotAnEmoji),
            ("🇰🇷🇯🇵", ParseEmojiError::NotAnEmoji),
            ("\u{200D}", ParseEmojiError::NotAnEmoji),
            ("👍\u{200D}", ParseEmojiError::NotAnEmoji),
            ("\u{200D}👍", ParseEmojiError::NotAnEmoji),
            ("\u{FE0F}", ParseEmojiError::NotAnEmoji),
            ("🇰", ParseEmojiError::NotAnEmoji),
            ("🏴\u{E0067}\u{E0062}", ParseEmojiError::NotAnEmoji),
            // The text's look of an emoji; tags that are no small letter or digit, and
            // too few of them.
            ("👍\u{FE0E}", ParseEmojiError::NotAnEmoji),
            ("🏴\u{E0020}\u{E007F}", ParseEmojiError::NotAnEmoji),
            ("🏴\u{E0041}\u{E007F}", ParseEmojiError::NotAnEmoji),
            ("🏴\u{E0067}\u{E0062}\u{E007F}", ParseEmojiError::NotAnEmoji),
            ("👍\u{0301}", ParseEmojiError::NotAnEmoji),
            // A dingbat's circled digit, and its arrow, are no emoji.
            ("❶", ParseEmojiError::NotAnEmoji),
            ("➓", ParseEmojiError::NotAnEmoji),
            ("➔", ParseEmojiError::NotAnEmoji),
            ("👩🏻‍❤️‍💋‍👨🏼", ParseEmojiError::TooLong),
        ] {
            assert_eq!(text.parse::<Emoji>(), Err(error), "{text:?}");
        }
    }

    #[test]
    fn an_emoji_is_read_and_written_as_its_text() {
        let reaction = Reaction {
            emoji: "🎉".parse().expect("an emoji"),
            user_ids: vec!["u".to_owned()],
        };
        let json = serde_json::to_value(&reaction).expect("written");
        assert_eq!(json, serde_json::json!({"emoji": "🎉", "userIds": ["u"]}));
        assert_eq!(
            serde_json::from_value::<Reaction>(json).expect("read"),
            reaction
        );
        assert!(serde_json::from_value::<Emoji>(serde_json::json!("ab")).is_err());
    }
}
