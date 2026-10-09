import { defineConfig } from "@playwright/test";
import { resolve } from "node:path";

// Used only by the owned discovery fixtures. Product configuration is untouched.
const testDir = process.env.FVOCI_GROUP_FIXTURE_DIR;
if (testDir === undefined || testDir === "")
  throw new Error("Missing owned discovery fixture directory");
export default defineConfig({
  testDir,
  outputDir: resolve(testDir, ".playwright-output"),
  fullyParallel: false,
  workers: 1,
  retries: 0,
  forbidOnly: true,
});
