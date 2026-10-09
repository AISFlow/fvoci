# FVOCI local libsql 0.9.30 patch

This is the maintained crates.io libsql 0.9.30 source at upstream VCS
`0653c5788d77ef16a97c56ff3e9fdc11717a72d9`, directory `libsql`.
It is a local FVOCI patch, not an upstream accepted release. See
`PROVENANCE.json` and `UPSTREAM-FILES.json` for immutable original inputs.

Original crate SHA-256:
`30fe980ac5693ed1f3db490559fb578885e913a018df64af8a1a46e1959a78df`.
That checksum identifies the unmodified archive, not this patched source.
All 70 archive files are retained; 69 are byte-identical. The only changed
upstream file is `src/errors.rs`: 23 inserted production lines add a borrowed
`Error::hrana_error_code` accessor under the existing `hrana` feature, and
135 inserted test lines supply five focused controls. Existing error variants,
formatting, conversions, transport, SQL and stream settlement are unchanged.
The SDK keeps responsibility for its private Hrana statement error types.
FVOCI compares the exposed machine code, never message text or HTTP JSON.
A code does not prove commit, rollback, or server Close acknowledgment.

`LICENSE.md` is the full 1069-byte MIT text recovered by ROOT from the same
pinned upstream VCS root; the published archive omitted that file. Its SHA-256
is `cecf589818a56c2da8a0a6cc82c8a215c9770988c033b6016c2b74bf79f38152`.
The original crate metadata, normalized manifest, original manifest, lockfile
and all dev-dependencies are preserved. No version or feature change is made.

Source adoption only: SDK compilation and five Rust controls, actual remote
FK failure and original-stream cleanup are NOTRUN. ROOT owns product patch
wiring, lock qualification, build/image notices, shared migration helper
integration, and separately allocated runtime acceptance.
