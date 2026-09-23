# Fixture provenance

No private customer documents are stored here.

Binary `*.v1` / `*.bin` files and `expectations.json` are produced by
`js/generate.mjs` using pinned **Yjs 13.6.32** and public TipTap packages
(`@tiptap/core` / `starter-kit` / `extension-table` / `extension-unique-id` /
`y-tiptap` 3.0.9). Product code must never import `js/` or spawn Node.

Inspected locally (not copied) from FVOCI source
`393795261322b916e588043cf94feca999175843`:

- fragment name `prosemirror`
- `Y.Doc({ gc: false })` / `Y.encodeStateAsUpdate`
- `COLLAB_STATE_ENCODING_V1 = 1`
- compact-gc revision restore requires `gc: false`
- mention-like attrs `entity` / `id` / `label` (generator uses a public TipTap
  Node with those names; it does not copy FVOCI editor source)

Preserve this notice with the binaries.
