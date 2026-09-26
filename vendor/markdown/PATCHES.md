# Vendored markdown-rs

- Upstream: <https://github.com/wooorm/markdown-rs>, crate `markdown` **1.0.0**
  (crates.io checksum `a5cab8f2cadc416a82d2e783a1946388b31654d391d1c7d92cc1f03e295b1deb`,
  git `1506572f9b406431402928f3a8b3df0b4ae2d8f5`). License: MIT (`license`, kept verbatim).
- Contents: the published crate's `src/`, `license`, `README.md` and `Cargo.toml` with the
  `[dev-dependencies]` and bench/test/example targets removed (none are shipped in the crate).
  `Cargo.lock` only pins `unicode-id` for the vendored unit tests.
- Wired in by `[patch.crates-io] markdown = { path = "vendor/markdown" }` in the root
  `Cargo.toml`; the dependency stays pinned to `=1.0.0`.

## Patch 1: `EditMap::add` lookup (only change)

Upstream `add_impl` scans every pending edit to find one at the same position, so each
insert is O(pending edits). The document tokenizer adds one edit per flow line and the GFM
table resolver several per cell, so tokenizing is quadratic in lines/cells on ordinary input.
The patch keeps a `BTreeMap` from position to entry, cleared by `consume`; the order of
entries, the merge rule (`before` prepends, otherwise appends) and `consume` are unchanged.
Upstream tracks the same problem in issue #218 and unmerged PR #217 (checked 2026-09-26,
no release contains a fix).

Release build, `to_mdast` with the product options (GFM + math), same host:

| Input | 1.0.0 | patched |
| --- | --- | --- |
| 131,072 blank lines | 3.4 s | 0.14 s |
| 65,536-line paragraph | 3.4 s | 0.21 s |
| 1 MiB of 3-line GFM tables | 270.5 s | 1.16 s |
| one 10,000-row × 5-column table (488 KB) | 8.5 s | 0.27 s |
| 1,000,000 × `[` | > 120 s (killed) | 0.89 s |
| 300 × 300 table | 15.9 s | 0.30 s |

Still super-linear after the patch (also slow or failing in the JS micromark oracle; bounded
by the `--internal-markdown` child timeout): 30,000 nested `*a ` (12.9 s), 50,000 nested
`> ` (15.5 s), 1,000-level indented list (11.1 s).

Tests: `util::edit_map::tests` compares the patched map with the verbatim upstream
`add_impl` (same positions, before/after merges, removals, no-op edits, index cleared by
`consume`): `cargo test --manifest-path vendor/markdown/Cargo.toml --locked --offline --lib`.
The product's oracle corpus (`compat/fixtures/markdown-oracle`) runs through this copy.

Rule: replace this directory with the crates.io release (drop the `[patch]`) as soon as an
upstream release contains the fix; do not add other local changes here.

```diff
--- a/src/util/edit_map.rs
+++ b/src/util/edit_map.rs
@@ -9,7 +9,7 @@
 //! through another tokenizer and inject the result.
 
 use crate::event::Event;
-use alloc::{vec, vec::Vec};
+use alloc::{collections::BTreeMap, vec, vec::Vec};
 
 /// Shift `previous` and `next` links according to `jumps`.
 ///
@@ -59,12 +59,17 @@
 pub struct EditMap {
     /// Record of changes.
     map: Vec<(usize, usize, Vec<Event>)>,
+    /// Position in `map` of the entry for each `at` (keeps `add` O(log n)).
+    index: BTreeMap<usize, usize>,
 }
 
 impl EditMap {
     /// Create a new edit map.
     pub fn new() -> EditMap {
-        EditMap { map: vec![] }
+        EditMap {
+            map: vec![],
+            index: BTreeMap::new(),
+        }
     }
     /// Create an edit: a remove and/or add at a certain place.
     pub fn add(&mut self, index: usize, remove: usize, add: Vec<Event>) {
@@ -116,33 +121,29 @@
         }
 
         self.map.truncate(0);
+        self.index.clear();
     }
 }
 
 /// Create an edit.
 fn add_impl(edit_map: &mut EditMap, at: usize, remove: usize, mut add: Vec<Event>, before: bool) {
-    let mut index = 0;
-
     if remove == 0 && add.is_empty() {
         return;
     }
 
-    while index < edit_map.map.len() {
-        if edit_map.map[index].0 == at {
-            edit_map.map[index].1 += remove;
-
-            if before {
-                add.append(&mut edit_map.map[index].2);
-                edit_map.map[index].2 = add;
-            } else {
-                edit_map.map[index].2.append(&mut add);
-            }
+    if let Some(&index) = edit_map.index.get(&at) {
+        edit_map.map[index].1 += remove;
 
-            return;
+        if before {
+            add.append(&mut edit_map.map[index].2);
+            edit_map.map[index].2 = add;
+        } else {
+            edit_map.map[index].2.append(&mut add);
         }
 
-        index += 1;
+        return;
     }
 
+    edit_map.index.insert(at, edit_map.map.len());
     edit_map.map.push((at, remove, add));
 }
```
