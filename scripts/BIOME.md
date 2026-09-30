# Web lint contract

`bun ci`, `bun run lint:fixtures`, and `bun run lint` use workspace-local Biome
2.5.14. The CLI, root manifest, Bun lock and packaged schema must stay at the
same reviewed version. No global CLI or runtime-image dependency is required.
The versioned website schema currently returns HTTP 404, so `biome.json` uses
the schema shipped in the exact locked package.

`lint` runs read-only `biome ci --error-on-warnings`. The Web workflow runs it
once in `web-checks`, whose failure reaches the required `web-ci-gate`.
Configuration, manifests, lock and scripts select conservative full CI.
Existing strict `tsc` and `vue-tsc` checks for both web and editor remain required.
Phase A installs the gate and inventories violations; it does not make the tree
green or authorize product formatting. Formatting, imports/rule fixes and
behavior changes require their separate ownership checkpoints and reviews.

Full SFC parsing and formatting are explicitly enabled. HTML uses strict
whitespace sensitivity, CSS accepts Tailwind directives, and import organization
keeps bare side-effect imports in their original order. No unsafe fixes or
repository-wide fix command run in CI.

## Exceptions and support limits

- Vue-only `noUndeclaredVariables` is disabled because 2.5.14 incorrectly treats
  scoped-slot bindings and imported/intersection props as undeclared. The minimal
  valid reproduction remains in `test_scoped_slots_and_imported_props_gap_is_reproduced`.
  Existing strict `vue-tsc` covers missing script/template names and components;
  the fixture suite proves both web and editor configurations reject each.
  Track [upstream slot issue #11230](https://github.com/biomejs/biome/issues/11230)
  and [proposed fix #11268](https://github.com/biomejs/biome/pull/11268).
  Remove the override only after a pinned, reviewed stable upgrade passes the
  valid repro with that rule enabled and preserves all invalid compiler probes.
- `noUnusedVariables` and `noUnusedImports` remain active in Vue. Template-only
  refs, imported components, props, emits, `defineSlots`, and `v-for` are tested.
  A name used only in `#[slotName]` is still falsely reported unused. The valid
  dynamic-slot reproduction and invalid missing-slot-name counterpart are checked
  by both compiler configurations. The Biome failure is retained as a known
  capability gap, not a passing support claim, a suppression or a product workaround.
  No current web/editor source uses dynamic slot-name syntax. Track
  [cross-language analysis #8590](https://github.com/biomejs/biome/issues/8590);
  reevaluate the repro on a reviewed stable upgrade before adding such a flow.
- Nuxt UI component/composable auto-imports are disabled in FVOCI's Vite options.
  The valid fixture imports the real installed UButton explicitly. Biome accepts
  unregistered template component names; `vue-tsc` rejects the corresponding
  auto-import probe. Any future global registration needs actual generated types.
- React-domain rules are inapplicable to `.vue` components and disabled only in
  the existing Vue override; React PDF export code retains its React checks.
  The fixture suite verifies Vue `useTemplateRef` passes and invalid React hooks fail.
- Bun's global is declared only for test/spec files. SwaggerUIBundle is declared
  only for its actual loader. Browser product files receive neither global.
- Biome is not the TypeScript compiler or an HTML sanitizer. Its promise rules
  also accept deliberate `void` and rejection handlers; behavioral review must
  reject using them or empty handlers merely to silence diagnostics.

## Coverage and exclusions

`files.includes` starts with `**`; all maintained TS/TSX/JS/MJS/JSON/CSS/HTML/Vue
files, including web, editor, i18n, settings and test code, are eligible. VCS ignore
files and Biome's supported-language/size checks still apply. The diagnostic report
must compare the actual verbose processed-file manifest to tracked paths; file
counts alone are not coverage proof. Biome does not check Rust, shell, Python,
Markdown, YAML or binary fixture contents. Existing tests and workflow validation
cover those separate contracts.

The force-excluded paths have source-specific reasons:

| Path | Reason |
| --- | --- |
| `**/node_modules`, `**/.codegraph` | Dependency installations and local development index |
| `vendor` | Unmodified third-party crate sources |
| `compat/fixtures` | Frozen cross-runtime oracle inputs and byte-sensitive expected output |
| `apps/web/src/generated/api.ts`, `apps/web/openapi.json` | Rust OpenAPI generator outputs verified independently in CI |
| `bun.lock`, `compat/js/package-lock.json`, `crates/collab-engine/js/package-lock.json` | Package-manager-generated locks, preserved in their native format |
| `src/http/api_docs_assets/swagger-ui-bundle.js`, `src/http/api_docs_assets/swagger-ui.css` | Vendored Swagger UI distribution assets |
| `src/collab/emoji_shortcodes.json`, `tests/fixtures/collab-derived/emoji_glyph_expected.json` | Generated by `scripts/generate-emoji-shortcodes.mjs` from the pinned emoji oracle |

Hand-authored test JSON, fixture generator code, package manifests and i18n JSON
remain checked. There is no product-path exclusion or diagnostic baseline.
