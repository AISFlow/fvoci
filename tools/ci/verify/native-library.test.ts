import { expect, test } from "bun:test";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { resolve, join } from "node:path";
import { tmpdir } from "node:os";
import { YAML } from "bun";
import { type Mapping } from "./rust-common.ts";
import {
  NATIVE_LIBRARY_FILE,
  NATIVE_LIBRARY_STEPS,
  verifyNativeLibraryExecution,
} from "./rust-native-library.ts";

const root = resolve(import.meta.dir, "../../..");

test("native import and body-save controls execute after sealed helper validation", () => {
  const workflow = YAML.parse(
    readFileSync(resolve(root, ".github/workflows/rust.yml"), "utf8"),
  ) as {
    jobs: { collaboration: { steps: { name?: string; run?: string }[] } };
  };
  const steps = workflow.jobs.collaboration.steps;
  const validate = steps.findIndex(
    (s) => s.name === "Validate and restore finished helper executables (no rebuild fallback)",
  );
  const run = steps.findIndex(
    (s) => s.name === "Native collaboration library controls (28 exact tests)",
  );
  expect(run).toBeGreaterThan(validate);
  expect(steps[run]?.run).toContain("native-library run --cohort collaboration");
});

test("every cohort step rejects missing, duplicated, masked, moved and inputless execution", () => {
  for (const [owner, expected] of NATIVE_LIBRARY_STEPS) {
    for (const mode of [
      "missing",
      "duplicate",
      "masked",
      "moved",
      "inputless",
      "changed-command",
      "before-validation",
    ]) {
      const workflow = YAML.parse(
        readFileSync(resolve(root, ".github/workflows/rust.yml"), "utf8"),
      ) as { jobs: Record<string, { steps: Mapping[] }> };
      const list = workflow.jobs[owner]?.steps;
      if (!list) throw new Error("missing fixture job");
      const at = list.findIndex((step) => step.name === expected.name);
      const found = list[at];
      if (!found) throw new Error("missing fixture step");
      if (mode === "missing") list.splice(at, 1);
      if (mode === "duplicate") list.push({ ...found });
      if (mode === "masked") found["continue-on-error"] = true;
      if (mode === "moved") {
        list.splice(at, 1);
        workflow.jobs.fast?.steps.push(found);
      }
      if (mode === "inputless") found.env = {};
      if (mode === "changed-command") found.run = String(found.run) + " || true";
      if (mode === "before-validation") {
        list.splice(at, 1);
        list.unshift(found);
      }
      expect(
        verifyNativeLibraryExecution(workflow.jobs, root).length,
        `${String(expected.name)} ${mode}`,
      ).toBeGreaterThan(0);
    }
  }
});

test("unknown, zero, duplicate or absent registry cannot become coverage", () => {
  const workflow = YAML.parse(
    readFileSync(resolve(root, ".github/workflows/rust.yml"), "utf8"),
  ) as { jobs: Mapping };
  expect(verifyNativeLibraryExecution(workflow.jobs, root)).toEqual([]);
  const dir = mkdtempSync(join(tmpdir(), "native-library-verify-"));
  const text = readFileSync(resolve(root, NATIVE_LIBRARY_FILE), "utf8");
  mkdirSync(join(dir, "xtask"));
  try {
    expect(verifyNativeLibraryExecution(workflow.jobs, dir).length).toBeGreaterThan(0);
    for (const changed of [
      "",
      text + text,
      text.replace(/\t[^\n]+/, "\tunknown::test"),
      text.split("\n").slice(1).join("\n"),
    ]) {
      writeFileSync(join(dir, NATIVE_LIBRARY_FILE), changed);
      expect(verifyNativeLibraryExecution(workflow.jobs, dir).length).toBeGreaterThan(0);
    }
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("renaming a second execution does not evade the one-cohort contract", () => {
  const workflow = YAML.parse(
    readFileSync(resolve(root, ".github/workflows/rust.yml"), "utf8"),
  ) as { jobs: Record<string, { steps: Mapping[] }> };
  const original = workflow.jobs.collaboration?.steps.find((step) =>
    String(step.run).includes("--cohort collaboration"),
  );
  if (!original) throw new Error("missing fixture execution");
  workflow.jobs.collaboration?.steps.push({ ...original, name: "renamed duplicate" });
  expect(verifyNativeLibraryExecution(workflow.jobs, root)).toContain(
    "rust: native library unregistered or duplicate execution is forbidden",
  );
});
