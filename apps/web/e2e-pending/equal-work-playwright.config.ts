import { defineConfig } from "@playwright/test";
import { readFileSync } from "node:fs";
import config from "./collab-playwright.config";
import { admin, member } from "./collab-helpers";
import { requiredFixtureInput, selectedSetupNeeded } from "./selected-backend-fixture";
import { equalWorkPreparedMode, parsePreparedEqualWork } from "./equal-work-prepared";

if (!process.env.FVOCI_E2E_EQUAL_WORK_DIR || !process.env.FVOCI_E2E_SELECTED_BACKEND)
  throw new Error("equal work requires the allocated normal-server collector");

if (
  equalWorkPreparedMode(
    process.env.FVOCI_E2E_EQUAL_WORK_PREPARED,
    process.env.FVOCI_E2E_SELECTED_BACKEND,
  )
) {
  if (selectedSetupNeeded()) throw new Error("prepared equal work cannot run setup");
  parsePreparedEqualWork(
    JSON.parse(readFileSync(requiredFixtureInput("FVOCI_E2E_TURSO_ACTOR_BINDING"), "utf8")),
    {
      backend: requiredFixtureInput("FVOCI_E2E_SELECTED_BACKEND"),
      flow: requiredFixtureInput("FVOCI_E2E_SELECTED_FLOW"),
      namespace: requiredFixtureInput("FVOCI_E2E_TURSO_NAMESPACE"),
      source: requiredFixtureInput("FVOCI_E2E_TURSO_SOURCE"),
      tree: requiredFixtureInput("FVOCI_E2E_TURSO_TREE"),
      targetSha256: requiredFixtureInput("FVOCI_E2E_TURSO_TARGET_SHA256"),
      ownerEmail: admin.email,
      memberEmail: member.email,
      workspaceSlug: admin.workspaceSlug,
    },
  );
}

export default defineConfig({
  ...config,
  testDir: ".",
  testMatch: "**/v060-equal-work.spec.ts",
  testIgnore: [],
  metadata: { ...config.metadata, purpose: "equal-work" },
  // These raw samples and reporter attachments contain no URLs or credentials.
  use: { ...config.use, trace: "off" },
});
