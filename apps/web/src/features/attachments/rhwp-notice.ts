/**
 * Public-notice entry for the rhwp WebAssembly module shipped with
 * `@rhwp/core`. The npm package carries only rhwp's own MIT LICENSE; the
 * Rust crates compiled into `rhwp_bg.wasm` are listed by upstream's
 * THIRD_PARTY_LICENSES.md, copied verbatim from the commit that npm's SLSA
 * provenance names for this version (also the native extract helper's pin).
 * That file is a crate/version/license table for the whole rhwp workspace,
 * not the full license texts of the wasm32 subset. Shared by vite.config.ts.
 */
export const RHWP_CORE_VERSION = "0.8.6";
export const RHWP_SOURCE_COMMIT = "e8800c8def63449808a4092798442652ed460552";
export const RHWP_THIRD_PARTY_FILE = "supplements/rhwp-e8800c8-THIRD_PARTY_LICENSES.md";
export const RHWP_THIRD_PARTY_SOURCE = `https://raw.githubusercontent.com/edwardkim/rhwp/${RHWP_SOURCE_COMMIT}/THIRD_PARTY_LICENSES.md`;
export const RHWP_THIRD_PARTY_SHA256 = "25baf4e05f26ab6009522d47843ac7247c1be64ae45658400227497106fda79b";

export function rhwpWasmNoticeTitle(version: string): string {
  if (version !== RHWP_CORE_VERSION) {
    throw new Error(`@rhwp/core ${version} installed; update the rhwp wasm notice (pinned ${RHWP_CORE_VERSION})`);
  }
  return `@rhwp/core ${version} rhwp_bg.wasm: third-party crate table (${RHWP_THIRD_PARTY_SOURCE})`;
}
