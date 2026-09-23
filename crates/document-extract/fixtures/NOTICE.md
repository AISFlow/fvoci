# Fixture provenance

No private customer documents are stored here.

## User-authored Hancom 12.30 samples

Copied from integration SHA `bea324300d133d0fa5b81880fef9ea50c132e451`
`compat/fixtures/sample.hwp` and `sample.hwpx` after the user confirmed they
personally opened the files in Hangul 12.30, checked them, and wrote the word
`안녕`. Expected body: `안녕`. Real CFB/ZIP containers. HWP has
`BodyText/Section0`. Default Scripts in the package must never be executed.

Filenames here: `user-hancom-12.30-안녕.hwp`, `user-hancom-12.30-안녕.hwpx`.

## Independent generated fixtures

Tests also call `document_extract::gen` to build format-valid HWP 5.0 CFB and
HWPX ZIP bytes at runtime (multi-section, table, Korean/emoji, empty, corrupt,
mismatch, limits). Expected strings are declared next to the generators.

Inspected locally (not copied otherwise) from FVOCI source
`393795261322b916e588043cf94feca999175843`:

- `packages/jobs/src/extract-text.ts` status/limit contracts
- `packages/jobs/test/hwp5-fixture.ts` CFB + raw-deflate BodyText layout
- `packages/core/src/zip-store.ts` zip entry/path/200MiB guards

Upstream rhwp (MIT, Edward Kim) at
`e8800c8def63449808a4092798442652ed460552` is a Cargo dependency, not vendored
in this tree. Preserve this notice if generators or samples are copied.
