//! Numbers and bodies made from a seed, the same on every run: a failing case names
//! its seed and can be made again.

use serde_json::{Map, Value, json};

/// The `SplitMix64` generator: a sequence of numbers from a seed.
#[derive(Debug, Clone)]
pub(crate) struct Random(u64);

impl Random {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub(crate) fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut mixed = self.0;
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        mixed ^ (mixed >> 31)
    }

    /// A number below `bound`, which is not zero.
    pub(crate) fn below(&mut self, bound: usize) -> usize {
        let bound = u64::try_from(bound).expect("a bound fits");
        usize::try_from(self.next() % bound).expect("below the bound")
    }

    pub(crate) fn chance(&mut self, one_in: usize) -> bool {
        self.below(one_in) == 0
    }

    pub(crate) fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }

    pub(crate) fn byte(&mut self) -> u8 {
        self.next().to_le_bytes()[0]
    }

    pub(crate) fn bytes(&mut self, length: usize) -> Vec<u8> {
        (0..length).map(|_| self.byte()).collect()
    }
}

/// Pieces of text: plain letters, Hangul, letters outside the basic plane, a sequence
/// joined into one sign, and what a text may hold that looks like structure.
const WORDS: [&str; 12] = [
    "a",
    "word",
    " ",
    "한글",
    "😀",
    "👩‍👩‍👧‍👦",
    "𝒳",
    "é",
    "\n",
    "\t",
    "<b>",
    "\"\\",
];

const MARKS: [&str; 5] = ["link", "bold", "italic", "strike", "code"];

fn words(random: &mut Random) -> String {
    (0..=random.below(3))
        .map(|_| *random.pick(&WORDS))
        .collect()
}

/// The marks of a run: a choice of the five, in the editor's order.
fn marks(random: &mut Random) -> Vec<Value> {
    let chosen = random.below(1 << MARKS.len());
    // Most text is plain.
    if random.chance(2) {
        return Vec::new();
    }
    MARKS
        .iter()
        .enumerate()
        .filter(|(index, _)| chosen & (1 << index) != 0)
        .map(|(_, mark)| match *mark {
            "link" => {
                json!({"type": "link", "attrs": {"href": format!("/boards/{}", random.below(9))}})
            }
            other => json!({"type": other}),
        })
        .collect()
}

/// What stands in a line: text runs, no two neighbours with the same marks (they
/// would be one), and nodes that are no text.
fn line(random: &mut Random) -> Vec<Value> {
    let mut content: Vec<Value> = Vec::new();
    for _ in 0..random.below(6) {
        let node = match random.below(9) {
            0 => json!({"type": "hardBreak"}),
            1 => json!({"type": "mention", "attrs": {
                "userId": format!("01930000-0000-7000-8000-00000000c00{}", 1 + random.below(8)),
            }}),
            2 => json!({"type": "inlineMath", "attrs": {"latex": words(random)}}),
            3 => json!({"type": "reference", "attrs": {
                "id": format!("01930000-0000-7000-8000-00000000b00{}", 1 + random.below(8)),
            }}),
            _ => {
                let marks = marks(random);
                let same_as_last = content.last().is_some_and(|last| {
                    last["type"] == "text"
                        && last
                            .get("marks")
                            .and_then(Value::as_array)
                            .map_or(&[][..], Vec::as_slice)
                            == marks.as_slice()
                });
                if same_as_last {
                    continue;
                }
                let mut text = Map::new();
                text.insert("type".to_owned(), json!("text"));
                if !marks.is_empty() {
                    text.insert("marks".to_owned(), Value::Array(marks));
                }
                text.insert("text".to_owned(), json!(words(random)));
                Value::Object(text)
            }
        };
        content.push(node);
    }
    content
}

/// A table that fills its grid: two or three columns, one to three rows, a row of
/// headers first or not, and at most one cell that spans two columns or two rows. Its
/// cells hold blocks nested no deeper than `depth` more levels.
fn table(random: &mut Random, depth: usize) -> Value {
    let columns = 2 + random.below(2);
    let rows = 1 + random.below(3);
    let header = random.chance(3);
    // The first cell spans two columns, or the last of the first row two rows.
    let (wide, tall) = match random.below(3) {
        0 => (true, false),
        1 => (false, rows > 1),
        _ => (false, false),
    };
    let lines = (0..rows)
        .map(|row| {
            let kind = if header && row == 0 {
                "tableHeader"
            } else {
                "tableCell"
            };
            let covered = usize::from((row == 0 && wide) || (row == 1 && tall));
            let width = columns - covered;
            let cells = (0..width)
                .map(|at| {
                    let attrs = match (row, at) {
                        (0, 0) if wide => Some(json!({"colspan": 2})),
                        (0, last) if tall && last + 1 == width => Some(json!({"rowspan": 2})),
                        _ => None,
                    };
                    with_content(kind, attrs, blocks(random, depth))
                })
                .collect();
            with_content("tableRow", None, cells)
        })
        .collect();
    with_content("table", None, lines)
}

/// A node with content, the member left out when there is none.
fn with_content(kind: &str, attrs: Option<Value>, content: Vec<Value>) -> Value {
    let mut node = Map::new();
    node.insert("type".to_owned(), json!(kind));
    if let Some(attrs) = attrs {
        node.insert("attrs".to_owned(), attrs);
    }
    if !content.is_empty() {
        node.insert("content".to_owned(), Value::Array(content));
    }
    Value::Object(node)
}

fn paragraph(random: &mut Random) -> Value {
    with_content("paragraph", None, line(random))
}

/// One to three blocks, nested no deeper than `depth` more levels.
fn blocks(random: &mut Random, depth: usize) -> Vec<Value> {
    (0..=random.below(3))
        .map(|_| block(random, depth))
        .collect()
}

/// The content of a list entry: a paragraph, then any blocks.
fn entry(random: &mut Random, depth: usize) -> Vec<Value> {
    let mut content = vec![paragraph(random)];
    if depth > 0 && random.chance(3) {
        content.extend(blocks(random, depth - 1));
    }
    content
}

fn block(random: &mut Random, depth: usize) -> Value {
    // Without room below, only what holds no blocks.
    let kinds = if depth > 0 { 11 } else { 6 };
    match random.below(kinds) {
        0 | 1 | 5 => paragraph(random),
        2 => with_content(
            "heading",
            Some(json!({"level": 1 + random.below(3)})),
            line(random),
        ),
        3 => {
            let language = random
                .chance(2)
                .then(|| json!({"language": *random.pick(&["rust", "mermaid", "c++"])}));
            let text = if random.chance(4) {
                Vec::new()
            } else {
                vec![json!({"type": "text", "text": words(random)})]
            };
            with_content("codeBlock", language, text)
        }
        4 => match random.below(4) {
            0 => json!({"type": "horizontalRule"}),
            1 => json!({"type": "blockMath", "attrs": {"latex": words(random)}}),
            2 => json!({"type": "file", "attrs": {"fileId": "file.pdf"}}),
            _ if random.chance(2) => json!({"type": "image", "attrs": {"fileId": "file.png"}}),
            _ => json!({"type": "image", "attrs": {"fileId": "file.png", "alt": words(random)}}),
        },
        6 => with_content("blockquote", None, blocks(random, depth - 1)),
        7 => {
            let entries = (0..=random.below(2))
                .map(|_| with_content("listItem", None, entry(random, depth - 1)))
                .collect();
            with_content("bulletList", None, entries)
        }
        8 => {
            let entries = (0..=random.below(2))
                .map(|_| with_content("listItem", None, entry(random, depth - 1)))
                .collect();
            // The start is written out, 1 among others, or left out: the stored form
            // leaves out 1, which an editor gives a list without one.
            let start = random.below(6);
            let attrs = (start < 5).then(|| json!({"start": start}));
            with_content("orderedList", attrs, entries)
        }
        _ => {
            let entries = (0..=random.below(2))
                .map(|_| {
                    let attrs = json!({"checked": random.chance(2)});
                    with_content("taskItem", Some(attrs), entry(random, depth - 1))
                })
                .collect();
            with_content("taskList", None, entries)
        }
    }
}

/// A callout of one of the tones, of blocks.
fn callout(random: &mut Random, depth: usize) -> Value {
    let tone = *random.pick(&["info", "tip", "warn", "danger"]);
    with_content(
        "callout",
        Some(json!({"tone": tone})),
        blocks(random, depth),
    )
}

/// A callout, or a toggle whose summary may be empty and whose content may hold a
/// table, a callout and, `nested` times more at most, a toggle besides its blocks.
fn toggle_or_callout(random: &mut Random, nested: usize) -> Value {
    if random.chance(2) {
        return callout(random, 2);
    }
    let mut hidden = blocks(random, 2);
    if random.chance(3) {
        hidden.push(table(random, 1));
    }
    if random.chance(3) {
        hidden.push(callout(random, 1));
    }
    if nested > 0 && random.chance(4) {
        hidden.push(toggle_or_callout(random, nested - 1));
    }
    with_content(
        "details",
        None,
        vec![
            with_content("detailsSummary", None, line(random)),
            with_content("detailsContent", None, hidden),
        ],
    )
}

/// A body in the form an editor stores, so that it comes back as it went: marks in
/// the editor's order, no neighbouring texts with the same marks, no empty `attrs` or
/// `content`, the start of an ordered list written out. Never one empty paragraph alone.
pub(crate) fn document(random: &mut Random) -> Value {
    loop {
        let mut content = blocks(random, 4);
        // A table stands only in the document itself, and in what a toggle hides.
        if random.chance(3) {
            let at = random.below(content.len() + 1);
            content.insert(at, table(random, 2));
        }
        // So do a toggle and a callout.
        if random.chance(3) {
            let at = random.below(content.len() + 1);
            content.insert(at, toggle_or_callout(random, 2));
        }
        if content != [json!({"type": "paragraph"})] {
            return json!({"type": "doc", "content": content});
        }
    }
}
