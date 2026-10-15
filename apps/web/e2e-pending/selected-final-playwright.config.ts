import { defineConfig } from "@playwright/test";
import { isAbsolute, join } from "node:path";
import { lstatSync } from "node:fs";
import { FINAL_SPEC, finalTitles, readFinalBinding } from "./selected-final-contract";

const { binding, sha256 } = readFinalBinding();
const baseURL = process.env.PLAYWRIGHT_BASE_URL;
const executablePath = process.env.FVOCI_E2E_FINAL_BROWSER_BIN;
const outputDir = process.env.FVOCI_E2E_RESULT_DIR;
if (
  !baseURL ||
  !executablePath ||
  !isAbsolute(executablePath) ||
  !outputDir ||
  !isAbsolute(outputDir)
)
  throw new Error("SELECTED_FINAL_ALLOCATION_REQUIRED");
const output = lstatSync(outputDir);
if (
  !output.isDirectory() ||
  output.isSymbolicLink() ||
  output.uid !== process.getuid?.() ||
  (output.mode & 0o777) !== 0o700
)
  throw new Error("SELECTED_FINAL_ALLOCATION_REQUIRED");
const origin = new URL(baseURL);
if (
  !["http:", "https:"].includes(origin.protocol) ||
  origin.username ||
  origin.password ||
  origin.pathname !== "/" ||
  origin.search ||
  origin.hash
)
  throw new Error("SELECTED_FINAL_ALLOCATION_REQUIRED");
if (process.env.CI && !process.versions.bun) throw new Error("Playwright must run under Bun in CI");

export default defineConfig({
  forbidOnly: true,
  testDir: ".",
  testMatch: FINAL_SPEC,
  workers: 1,
  retries: 0,
  metadata: {
    selectedFinal: true,
    selectedBackend: binding.backend,
    selectedFlow: binding.flow,
    source: binding.source,
    tree: binding.tree,
    browser: binding.browser,
    suite: binding.suite,
    actorBindingSha256: sha256,
    titles: finalTitles(binding),
  },
  outputDir: join(outputDir, "selected-final-playwright-output"),
  use: {
    browserName: binding.browser,
    baseURL,
    launchOptions: {
      executablePath,
      env: Object.fromEntries(
        ["PATH", "LANG", "TZ", "TMPDIR"].flatMap((name) => {
          const value = process.env[name];
          return value === undefined ? [] : [[name, value]];
        }),
      ),
    },
    trace: "off",
    screenshot: "off",
    video: "off",
  },
});
