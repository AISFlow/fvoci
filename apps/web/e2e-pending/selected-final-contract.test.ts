import { expect, test } from "bun:test";
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import ts from "typescript";
import {
  assertFinalInventory,
  boundaryTitle,
  browsers,
  flows,
  PRODUCT_TITLES,
  readFinalBinding,
  validateFinalBinding,
} from "./selected-final-contract";

const namespace = "tui-0123456789abcdef0123";
const actor = (name: string, index: number) => ({
  userId: `00000000-0000-4000-8000-00000000000${String(index)}`,
  email: `${namespace}-${name}@example.invalid`,
  password: "test-only-password",
});
const value = {
  schema: 1,
  backend: "libsql-remote",
  source: "a".repeat(40),
  tree: "b".repeat(40),
  browser: "chromium",
  flow: "on",
  suite: "products",
  namespace,
  workspaceId: "00000000-0000-4000-8000-000000000001",
  workspaceSlug: namespace,
  setupNeeded: false,
  schemaCurrent: true,
  commit: "confirmed",
  lifecycleDrain: "confirmed",
  leases: 0,
  owner: actor("owner", 2),
  member: actor("member", 3),
  outsider: actor("outsider", 4),
};
const env = {
  FVOCI_E2E_SELECTED_BACKEND: "libsql-remote",
  FVOCI_E2E_SELECTED_FLOW: "on",
  FVOCI_E2E_SELECTED_SOURCE: value.source,
  FVOCI_E2E_SELECTED_TREE: value.tree,
  FVOCI_E2E_FINAL_BROWSER: "chromium",
  FVOCI_E2E_FINAL_SUITE: "products",
  FVOCI_E2E_TURSO_NAMESPACE: namespace,
};

test("mismatched source, backend, browser, flow or actor cannot select the normal server", () => {
  for (const mutation of [
    { browser: "safari" },
    { flow: "OFF" },
    { source: "c".repeat(40) },
    { tree: "" },
    { backend: "sqlite" },
    { leases: 1 },
    { setupNeeded: true },
    { lifecycleDrain: "unknown" },
    { member: value.owner },
    { owner: { ...value.owner, email: "foreign@example.invalid" } },
  ])
    expect(() => validateFinalBinding({ ...value, ...mutation }, env)).toThrow();
  expect(() => validateFinalBinding(value, { ...env, FVOCI_E2E_PENDING: "1" })).toThrow();
  expect(() =>
    validateFinalBinding(value, { ...env, FVOCI_E2E_SELECTED_SOURCE: undefined }),
  ).toThrow();
});

test("exact case inventory refuses zero, duplicate, omitted, reordered or unexpected cases", () => {
  const binding = validateFinalBinding(value, env);
  assertFinalInventory([...PRODUCT_TITLES], binding);
  for (const wrong of [
    [],
    PRODUCT_TITLES.slice(1),
    [...PRODUCT_TITLES, PRODUCT_TITLES[0]],
    [...PRODUCT_TITLES].reverse(),
    [...PRODUCT_TITLES, "pending fixture"],
  ])
    expect(() => {
      assertFinalInventory(wrong, binding);
    }).toThrow();
});

test("six browser-flow boundary selections keep products out of OFF and non-Chromium runs", () => {
  const titles = [];
  for (const browser of browsers)
    for (const flow of flows) {
      const input = { ...value, browser, flow, suite: "boundary" };
      const selection = {
        ...env,
        FVOCI_E2E_FINAL_BROWSER: browser,
        FVOCI_E2E_SELECTED_FLOW: flow,
        FVOCI_E2E_FINAL_SUITE: "boundary",
      };
      const binding = validateFinalBinding(input, selection);
      const title = boundaryTitle(browser, flow);
      titles.push(title);
      assertFinalInventory([title], binding);
      expect(() => {
        assertFinalInventory([...PRODUCT_TITLES], binding);
      }).toThrow();
      if (browser !== "chromium" || flow !== "on")
        expect(() =>
          validateFinalBinding(
            { ...input, suite: "products" },
            { ...selection, FVOCI_E2E_FINAL_SUITE: "products" },
          ),
        ).toThrow();
    }
  expect(new Set(titles).size).toBe(6);
});

test("private actor input is reread and rejects public permissions or changed binding", () => {
  const directory = mkdtempSync(join(tmpdir(), "selected-final-"));
  const path = join(directory, "actors.json");
  try {
    writeFileSync(path, JSON.stringify(value), { mode: 0o600 });
    const selected = { ...env, FVOCI_E2E_FINAL_ACTOR_BINDING: path };
    const original = readFinalBinding(selected);
    chmodSync(path, 0o644);
    expect(() => readFinalBinding(selected)).toThrow();
    chmodSync(path, 0o600);
    writeFileSync(path, JSON.stringify({ ...value, source: "d".repeat(40) }));
    expect(() => readFinalBinding(selected)).toThrow();
    writeFileSync(
      path,
      JSON.stringify({ ...value, outsider: { ...value.outsider, password: "different" } }),
    );
    expect(readFinalBinding(selected).sha256).not.toBe(original.sha256);
  } finally {
    rmSync(directory, { recursive: true });
  }
});

function basePlaywrightOnly(source: string): void {
  const ast = ts.createSourceFile("selected.ts", source, ts.ScriptTarget.Latest, true);
  let base = 0;
  for (const statement of ast.statements) {
    if (ts.isImportDeclaration(statement) && ts.isStringLiteral(statement.moduleSpecifier)) {
      const imports = statement.importClause?.namedBindings;
      if (imports && ts.isNamedImports(imports))
        for (const name of imports.elements) {
          if ((name.propertyName ?? name.name).text === "test") {
            if (statement.moduleSpecifier.text !== "@playwright/test")
              throw Error("self-start fixture");
            base++;
          }
        }
    }
  }
  if (base !== 1 || /\bcollabApp\b|\bstartOwnedServer\b|\btest\.extend\b/.test(source))
    throw Error("self-start fixture");
}

test("acceptance uses base Playwright and cannot consume self-starting collaboration fixtures", () => {
  const source = readFileSync(join(import.meta.dir, "selected-final-acceptance.ts"), "utf8");
  basePlaywrightOnly(source);
  expect(() => {
    basePlaywrightOnly(source.replace('from "@playwright/test"', 'from "./collab-helpers"'));
  }).toThrow();
  expect(() => {
    basePlaywrightOnly(source + "\nconst { collabApp } = fixture;");
  }).toThrow();
});
