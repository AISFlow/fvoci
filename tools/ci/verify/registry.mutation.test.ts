import { describe, expect, test } from "bun:test";
import { readFileSync, readdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { verifyWorkflows } from "../verify-workflows.ts";
import { contextFromTexts } from "./load.ts";
import { MUTATION_CASES } from "./registry.cases.ts";

const root = resolve(import.meta.dir, "../../..");
const directory = join(root, ".github", "workflows");
const texts: Record<string, string> = Object.fromEntries(
  readdirSync(directory)
    .filter((name) => /\.ya?ml$/.test(name))
    .map((name) => [name, readFileSync(join(directory, name), "utf8")]),
);

function errorsWith(file: string, text: string): string[] {
  return verifyWorkflows(contextFromTexts(root, { ...texts, [file]: text }));
}

describe("RegistryMutationCliTest (workflow mutations fail closed)", () => {
  test("the unmutated workflows pass", () => {
    expect(verifyWorkflows(contextFromTexts(root, texts))).toEqual([]);
  });

  test("there are enough distinct mutations to cover every boundary", () => {
    expect(MUTATION_CASES.length).toBeGreaterThanOrEqual(150);
    expect(new Set(MUTATION_CASES.map((c) => c.name)).size).toBe(MUTATION_CASES.length);
  });

  for (const mutation of MUTATION_CASES) {
    test(`${mutation.file}: ${mutation.name}`, () => {
      const original = texts[mutation.file] ?? "";
      const changed = mutation.edit(original);
      expect(changed).not.toBe(original);
      const errors = errorsWith(mutation.file, changed);
      expect(
        errors.some((error) => error.includes(mutation.needle)),
        errors.join("\n"),
      ).toBe(true);
    });
  }
});
