# Fonts for PDF export

`scripts/document-convert` embeds these TTFs when it renders a document to PDF
(Korean body text, CJK monospace code, emoji). Without them Korean text in
exported PDFs has no glyphs. The files are byte-identical to the source
repository's `packages/ui/fonts` (source commit `39379526`), where the pinned
upstream URLs and reproduction steps are recorded.

All three families are distributed under the SIL Open Font License 1.1; the
licence texts sit beside the binaries and must ship with them.

| File                    |    Bytes | SHA-256                                                            | Licence               |
| ----------------------- | -------: | ------------------------------------------------------------------ | --------------------- |
| `NotoSansKR.ttf`        | 10414588 | `194018e6b2b293a7964f037b25c0249ce1418bc9ab3c971060a03aa57861e252` | `NotoSansKR-OFL.txt`  |
| `NotoSansMonoCJKkr.ttf` | 21351792 | `66ce9752303afb91dbc8276de50e5e8c309eb1f497c4f3a199dc12aefac4b87d` | `NotoSansCJK-OFL.txt` |
| `NotoEmoji.ttf`         |  1982596 | `de6c18832938afc99caf132b39d6a30a19bac7f2e812e28db2535b4608d27551` | `NotoEmoji-OFL.txt`   |
