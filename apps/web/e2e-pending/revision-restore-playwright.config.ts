import { defineConfig } from "@playwright/test";
import config from "../playwright.config";

export default defineConfig({
  ...config,
  testDir: ".",
  testMatch: "**/v060-sqlite-revision-restore.spec.ts",
});
