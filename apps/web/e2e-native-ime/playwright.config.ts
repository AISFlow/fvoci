import { defineConfig } from "@playwright/test";
if (!process.env.FVOCI_NATIVE_IME_SESSION)
  throw new Error("Invoke run-private.sh explicitly; this lane requires OS IBus Hangul");
export default defineConfig({
  testDir: ".",
  testMatch: "editor.spec.ts",
  workers: 1,
  retries: 0,
  use: { baseURL: process.env.PLAYWRIGHT_BASE_URL },
});
