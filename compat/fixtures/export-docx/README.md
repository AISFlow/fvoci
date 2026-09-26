# DOCX export fixtures

Hand-written Tiptap bodies for the Rust DOCX writer (`src/documents/docx.rs`), on top of the
53 `../markdown-oracle` bodies. `tests/docx_export_process.rs` runs all of them through the
`--internal-markdown --op tiptap-to-docx` child and opens every result with docx-rs's reader and
the product's OOXML importer.

| File | Covers |
| --- | --- |
| h01 | marks (bold, italic, strike, code, underline, highlight, links) |
| h02 | nested bullet/ordered lists, restarts |
| h03 | task lists |
| h04 | ragged table |
| h05 | callouts and quotes |
| h06 | code, math, mermaid |
| h07 | atoms: mentions, emoji, attachments, embeds |
| h08 | details, horizontal rule, headings |
| h09 | Korean, emoji, NFD, ZWSP, NBSP |
| h10 | fallbacks, unsafe links, control characters |
| h11 | a table cell whose first or last block is a nested table |
| h12 | link hrefs with spaces, Korean, quotes and other characters RFC 3986 forbids |

## Intentional differences from the TS export

The TS export (`packages/editor/src/export/docx.ts`: Tiptap → Markdown → `@m2d/core`) is the
oracle; the Rust writer keeps its product meaning without the Markdown round trip. These
differences are deliberate (numbers refer to the markdown-oracle `NN`/`gNN` and `hNN` fixtures).

| # | TS | Rust | Fixtures |
| --- | --- | --- | --- |
| D1 | Heading styles shifted by one (a body H1 is a second `Title`) | `HeadingN` = level N | all headings |
| D2 | Task item: bullet plus a clickable `w14:checkbox` | ☑/☐ glyph, no bullet; not clickable | 04, 27, g04, g20, h03 |
| D3 | Highlight exported as literal `==x==` | real highlight | 14, 23, g16, h01 |
| D4 | Underline dropped | underline | h01 |
| D5 | Cells past the header width dropped | all cells, rows padded to the widest | 05, h04 |
| D6 | Display math as `$$ … $$` text, `\` eaten by Markdown escapes | LaTeX source verbatim, monospace | 10, 11, 26, g11, h06 |
| D7 | Whitespace trimmed/collapsed | stored whitespace (text `\n`/`\t` → space) | 07, 13, h09 |
| D8 | Continuation paragraphs in a list item numbered as new items | indented, unnumbered | 03, 26, h02 |
| D9 | Empty list item dropped | empty numbered/bulleted item kept | 03 |
| D10 | Stored literal Markdown re-parsed (`*x*` → italic, `[a](b)` → link) | literal text, marks from the body | 07, 16, 23, g16, h01 |
| D11 | Heading text with a newline split into heading + paragraph | one heading | 18 |
| D12 | Inline HTML in a details summary parsed | literal text | 25, g13 |
| D13 | File attachment linked to `attachment:<id>` (dead outside the app) | name text only | 08, g15, h07 |
| D14 | Unsafe/relative hrefs as hyperlinks (`javascript:`, `data:`, `//x`, `/docs/1`) | text only | 29, g07, g20, h10, h12 |
| D15 | C0 controls written raw (`word/document.xml` not well-formed) | NUL → U+FFFD, others dropped | 28, h10 |
| D16 | Raw href as the relationship target | a valid URI: RFC 3986-illegal bytes (space, non-ASCII, quotes, angle brackets, backslash, caret, backtick, braces, pipe, a `%` without two hex digits) percent-encoded | h12 |

The package has no directory entries (OPC parts only), and a table cell always ends with a
paragraph, also after a nested table (TS flattened nested tables to text).
