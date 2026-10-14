import { defineConfig } from "@playwright/test";
import config from "./collab-playwright.config";

if (!process.env.FVOCI_E2E_EQUAL_WORK_DIR || !process.env.FVOCI_E2E_SELECTED_BACKEND)
  throw new Error("equal work requires the allocated normal-server collector");

export default defineConfig({
  ...config,
  testDir: ".",
  testMatch: "**/v060-equal-work.spec.ts",
  testIgnore: [],
  metadata: { ...config.metadata, purpose: "equal-work" },
  // These raw samples and reporter attachments contain no URLs or credentials.
  use: { ...config.use, trace: "off" },
});
