import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { formatWebDefaultTargets, prettierTargetMatches, shellWords } from "../ci/planner/paths.ts";

const root = resolve(import.meta.dir, "../..");
const read = (path: string) => readFileSync(resolve(root, path), "utf8");
const scripts = (JSON.parse(read("apps/web/package.json")) as { scripts: Record<string, string> })
  .scripts;

test("standing web lint and format commands cover the Keycloak harness", () => {
  const source = "tools/keycloak/kc-e2e.ts";
  for (const script of ["scripts/lint-web.sh", "scripts/format-web.sh"]) {
    expect(
      formatWebDefaultTargets(read(script)).some((target) => prettierTargetMatches(target, source)),
    ).toBe(true);
  }
  expect(
    formatWebDefaultTargets(read("scripts/format-web.sh")).some((target) =>
      prettierTargetMatches(target, "tools/keycloak/tsconfig.json"),
    ),
  ).toBe(true);
  const eslint = JSON.parse(read("apps/web/tsconfig.eslint.json")) as { include: string[] };
  expect(
    eslint.include.some((target) => prettierTargetMatches(target.replace("../../", ""), source)),
  ).toBe(true);
});

test("standing web typecheck and unit commands run the Keycloak project", () => {
  expect(shellWords(scripts["typecheck:tools"] ?? "")).toContain(
    "../../tools/keycloak/tsconfig.json",
  );
  expect(shellWords(scripts.test ?? "")).toContain("../../tools/keycloak/");
});

test("standing web checks cover the compat generator and document dependency check", () => {
  for (const script of ["scripts/lint-web.sh", "scripts/format-web.sh"]) {
    for (const source of ["compat/fixtures/gen.ts", "compat/fixtures/gen.test.ts"]) {
      expect(
        formatWebDefaultTargets(read(script)).some((target) =>
          prettierTargetMatches(target, source),
        ),
      ).toBe(true);
    }
  }
  expect(shellWords(scripts["typecheck:tools"] ?? "")).toContain(
    "../../compat/fixtures/tsconfig.json",
  );
  expect(shellWords(scripts.test ?? "")).toContain("../../compat/fixtures/gen.test.ts");
  expect(shellWords(scripts.test ?? "")).toContain("../../tools/ci/document-client-deps.test.ts");
});
