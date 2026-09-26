# PPTX export: intentional differences from the TS export

The PPTX writer (`src/documents/pptx.rs`) renders the shared export model with the layout of
`packages/editor/src/export/pptx.ts` (pptxgenjs): 10 × 5.625 in slides, boxes at x = 0.5 in,
9 in wide, the `estimateH` line estimate, `MIN_H` slide breaks, a top-level horizontal rule opens a
slide, the title as the first (28 pt) heading. It uses the same fixtures as the DOCX/PDF exports
(`compat/fixtures/markdown-oracle/*.json`, `compat/fixtures/export-docx/*.json`, 65 in all); the
comparison against the Node helper is recorded in the export PPTX evidence report (slide count,
per-slide text, table cells, list levels, well-formedness).

The differences below are deliberate; everything else (text, order, slides) follows the TS output.

| # | TS output | Rust | Fixtures |
| --- | --- | --- | --- |
| P1 | Nested list items flattened into their parent item's text at level 0; a `\n` inside a list item's text becomes a second bulleted paragraph | nested items are paragraphs one level deeper (`lvl`), with their own bullet/number; one bullet per item | 02, 03, 26, 27, g03, g20, h02, 05 |
| P2 | Task lists fall through to plain paragraphs (no checkbox) | `☑ `/`☐ ` before the item text, no bullet | 04, 27, g04, g20, h03 |
| P3 | Every ordered item is its own box numbered from 1 | `buAutoNum startAt` = the item's position | 03, 26 |
| P4 | Ragged table rows written as they are | rows padded to the widest row (a PresentationML table needs a full grid) | 05, h04 |
| P5 | Embeds print their bare `ref`; a mention without a label prints `@` | `[[doc:ref]]` / `[[task:ref]]` placeholder (URL embeds as a link), empty-label mentions dropped (shared export model, as DOCX/PDF) | 09, g09, h07 |
| P6 | `details` summary dropped | summary as a bold line before the body | 13, 25, g13, h08 |
| P7 | A table inside a quote, callout, list item or cell becomes its cells' text run together | one line per row, cells joined with ` \| ` | 22, h11 |
| P8 | Block math/Mermaid/code inside a list item or quote dropped (atoms have no child text) | kept as mono lines | 26 |
| P9 | C0 control characters written raw → slide XML not well-formed | NUL → U+FFFD, other controls dropped | 28, 31, h10 |
| P10 | A block taller than the rest of the slide is shrunk (`fit: shrink`) into it; tables drawn past the slide bottom, never split | text continues in a box on the next slide(s); table rows that do not fit continue in a table on the next slide with the header row repeated | 27 (header repeated), 1 MiB bodies |
| P11 | Quotes: one indented box; callouts: each child a separate full-width box | quotes and callouts one indented box with a coloured left bar (callouts also a background) | 10 (TS 2 slides, Rust 1), 22 |
| P12 | Marks ignored, links plain text | bold/italic/underline/strike/highlight/code font kept; `http`/`https`/`mailto` links as hyperlinks (RFC 3986-encoded targets, as DOCX/PDF); other schemes text only | 01, 07, 14, g07, h01, h12 |
| P13 | Each box extends to the slide bottom (`h = rest`) | box height = the estimated text height (boxes do not overlap) | all (visual only) |
| P14 | `image` nodes and attachments print `name`, else `title`/`fileName`/`src` | attachment `name` (the model's attachment; stored bodies use `attachment` nodes, no fixture has `image`) | 08, g15, h07 (equal: names set) |
| P15 | Package: pptxgenjs parts (notes master/slides, two themes, directory entries) | one master, one blank layout, one theme, no notes, no directory entries; `dc:title` = the visible title | all |
