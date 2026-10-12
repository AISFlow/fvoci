/**
 * Public-notice entry for the rhwp WebAssembly module shipped with
 * `@rhwp/core`. The npm package carries only rhwp's own MIT LICENSE, which
 * build.license already takes. The supplement concatenates the exact-version
 * license files of every crate in rhwp's wasm32 default-feature normal-edge
 * graph (`cargo tree --target wasm32-unknown-unknown --edges normal --locked`
 * at the commit npm's provenance names for this version) plus the Volexity
 * BSD-3-Clause notice for code rhwp incorporates. Shared by vite.config.ts.
 */
export const RHWP_CORE_VERSION = "0.8.7";
export const RHWP_SOURCE_COMMIT = "1a76570e833917d15817415a53c09ad61ab3203f";
export const RHWP_THIRD_PARTY_FILE = "supplements/rhwp-1a76570-wasm32-NOTICES.txt";
export const RHWP_THIRD_PARTY_SHA256 =
  "fd84144f0cf23e21e02b1c1406e6f1a441d1b33d0c8bd95ccfec3381c7114d4d";
/** Crates in the resolved graph, excluding rhwp's own MIT workspace crates. */
export const RHWP_WASM_CRATE_COUNT = 140;

export function rhwpWasmNoticeTitle(version: string): string {
  if (version !== RHWP_CORE_VERSION) {
    throw new Error(
      `@rhwp/core ${version} installed; update the rhwp wasm notice (pinned ${RHWP_CORE_VERSION})`,
    );
  }
  return `@rhwp/core ${version} rhwp_bg.wasm: third-party notices (edwardkim/rhwp ${RHWP_SOURCE_COMMIT})`;
}
