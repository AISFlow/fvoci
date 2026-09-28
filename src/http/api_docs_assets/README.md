# Vendored Swagger UI (`/api/docs`)

Unmodified files from the npm package `swagger-ui-dist` **5.33.0**, the version the
original server pinned (`apps/server/package.json`, `swagger-ui-dist: "5.33.0"`).
They are embedded in `fvoci-server` with `include_bytes!` and served only behind
the session guard at `/api/docs/static/*` (`src/http/routes/api_docs.rs`). No npm
dependency is added: the package depends on `@scarf/scarf`, whose install script
reports installs.

- Tarball: `https://registry.npmjs.org/swagger-ui-dist/-/swagger-ui-dist-5.33.0.tgz`
- Tarball integrity: `sha512-wpdK+m6BU5yj6pmUdMskZVTSWYG4DLglAx3sIhylloY37i8O37IrH+YEpqdXNfpaTGxILRBFzUqLF2jKqbfI7A==`
  (same as the original `bun.lock` entry and the registry `dist.integrity`)
- License: Apache-2.0 (`LICENSE`, `NOTICE`); bundled third-party notices in
  `swagger-ui-bundle.js.LICENSE.txt`

| File | Bytes | SHA-256 |
| --- | ---: | --- |
| `swagger-ui-bundle.js` | 1585988 | `62df541529080464a7660adc793eab7128c6193ce3be24ddc1e0e0a4a63edc2f` |
| `swagger-ui.css` | 186154 | `1ac324f7dcd27e4b9386b4bd6421271ec147e922a22c05ba24b11515e9aa6321` |
| `swagger-ui-bundle.js.LICENSE.txt` | 4442 | `63818894e4b04cd0e3180d9cb20761e227a939121e7484f8e1d528227c756f89` |
| `LICENSE` | 11358 | `cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30` |
| `NOTICE` | 55 | `0d20d1adef18aee3f40dd258172155521ce702ac445cb5f7b7d60ed32dad2fb2` |

The two served files are pinned by a unit test (`api_docs::tests`). To upgrade,
replace every file from the new tarball, verify its integrity against the
registry, and update this table, the test hashes and `SWAGGER_UI_VERSION`.
