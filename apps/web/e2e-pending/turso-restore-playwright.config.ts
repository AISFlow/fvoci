import { defineConfig } from "@playwright/test";
import config from "./collab-playwright.config";

export default defineConfig({
  ...config,
  testDir: ".",
  // An explicit acceptance suffix leaves generic pending discovery unchanged.
  testMatch: "**/v060-turso-restore.acceptance.ts",
  testIgnore: [],
  retries: 0,
  workers: 1,
  use: { ...config.use, trace: "off" },
});
