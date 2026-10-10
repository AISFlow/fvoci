import { describe, expect, test } from "bun:test";
import { readFileSync, readdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { verifyWorkflows } from "../verify-workflows.ts";
import { contextFromTexts, isMapping, type Mapping, type VerifyContext } from "./load.ts";
import { RUST1_MUTATIONS } from "./rust1-mutations.ts";

// The composed CLI check list reaches every Rust suite and web check through
// its slot, so each part-1 rust.yml mutation is reported end to end.
const root = resolve(import.meta.dir, "../../..");
const directory = join(root, ".github", "workflows");
const texts: Record<string, string> = Object.fromEntries(
  readdirSync(directory)
    .filter((name) => /\.ya?ml$/.test(name))
    .map((name) => [name, readFileSync(join(directory, name), "utf8")]),
);

function rustContext(): { ctx: VerifyContext; rust: Mapping } {
  const ctx = contextFromTexts(root, texts);
  const rust = ctx.workflows["rust.yml"];
  if (!isMapping(rust)) throw new Error("fixture rust.yml missing");
  return { ctx, rust };
}

describe("composed verify-workflows", () => {
  test("the unmutated repository passes every slot", () => {
    expect(verifyWorkflows(rustContext().ctx)).toEqual([]);
  });

  for (const mutation of RUST1_MUTATIONS) {
    test(mutation.name, () => {
      const { ctx, rust } = rustContext();
      mutation.mutate(rust);
      const errors = verifyWorkflows(ctx);
      expect(
        errors.some((error) => error.includes(mutation.needle)),
        errors.join("\n"),
      ).toBe(true);
    });
  }

  test("the binary handoff stays silent while rust.yml job ids are invalid", () => {
    const fastCache = "fast server cache must retain exact pinned restore/save";
    const { ctx, rust } = rustContext();
    const jobs = rust["jobs"] as Mapping;
    const fast = jobs["fast"] as Mapping;
    fast["steps"] = (fast["steps"] as Mapping[]).filter(
      (step) => step["name"] !== "Restore server build outputs",
    );
    expect(verifyWorkflows(ctx).filter((error) => error.includes(fastCache))).toHaveLength(1);
    jobs["bad id"] = { "runs-on": "ubuntu-26.04", steps: [] };
    const errors = verifyWorkflows(ctx);
    expect(errors).toContain("rust: invalid job id");
    expect(errors.filter((error) => error.includes(fastCache))).toEqual([]);
  });
});
