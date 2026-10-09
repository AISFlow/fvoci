import { expect, test } from "bun:test";
import { ESLint, Linter } from "eslint";
import playwright from "eslint-plugin-playwright";
import { resolve } from "node:path";

const rule = "playwright/prefer-web-first-assertions";
const root = resolve(import.meta.dir, "../../../..");

function lint(source: string) {
  return new Linter().verify(`import { expect } from "@playwright/test";\n${source}`, {
    plugins: { playwright },
    rules: { [rule]: "error" },
  });
}

for (const [name, source] of [
  ["textContent snapshot", 'expect(await editor.textContent()).toBe("하");'],
  ["innerText snapshot", "expect(await editor.innerText()).toBe(beforeRace);"],
  [
    "captured attribute truthiness",
    'const href = await link.getAttribute("href"); expect(href).toBeTruthy();',
  ],
  [
    "captured exact QR path",
    'const path = await qr.getAttribute("d"); expect(path).toBe(expectedPath);',
  ],
] as const) {
  test(`web-first rule rejects ${name}`, () => {
    const messages = lint(source);
    expect(messages).toHaveLength(1);
    expect(messages[0]).toMatchObject({ ruleId: rule, severity: 2 });
  });
}

for (const [name, source] of [
  ["raw textContent polling", 'await expect.poll(() => editor.textContent()).toBe("하");'],
  ["raw innerText polling", "await expect.poll(() => editor.innerText()).toBe(beforeRace);"],
  ["nonempty attribute", 'await expect(link).toHaveAttribute("href", /[\\s\\S]+/);'],
  ["exact QR path", 'await expect(qr).toHaveAttribute("d", expectedPath);'],
  ["zero hostile elements", "await expect(hostile).toHaveCount(0);"],
] as const) {
  test(`web-first rule accepts ${name}`, () => {
    expect(lint(source)).toEqual([]);
  });
}

test("nonempty attribute preserves string truthiness, including whitespace", () => {
  for (const value of [null, "", " ", "\n", "\r\n", "attachment", "한글"] as const) {
    expect(value !== null && /[\s\S]+/.test(value)).toBe(Boolean(value));
  }
});

test("E2E specs enable only this Playwright rule at error severity", async () => {
  const eslint = new ESLint({ cwd: root });
  for (const file of [
    "apps/web/e2e-native-ime/editor.spec.ts",
    "apps/web/e2e/task-archive-persist-flow.spec.ts",
    "apps/web/e2e/v050-editor-modes.spec.ts",
  ]) {
    const config = (await eslint.calculateConfigForFile(file)) as Linter.Config;
    const rules = Object.fromEntries(
      Object.entries(config.rules ?? {}).filter(([name]) => name.startsWith("playwright/")),
    );
    expect(rules).toEqual({ [rule]: [2] });
  }
});

test("E2E helpers and unit tests do not enable the Playwright rule", async () => {
  const eslint = new ESLint({ cwd: root });
  for (const file of [
    "apps/web/e2e/viewer-app.ts",
    "apps/web/src/build/playwright-await-assertions.test.ts",
    "packages/editor/src/vue/block-gutter.test.ts",
  ]) {
    const config = (await eslint.calculateConfigForFile(file)) as Linter.Config;
    expect(config.rules?.[rule]).toBeUndefined();
  }
});
