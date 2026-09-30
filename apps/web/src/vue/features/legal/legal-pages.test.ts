import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";

const dir = import.meta.dirname;

function source(file: string): string {
  return readFileSync(path.join(dir, file), "utf8");
}

await test("the Vue legal page reads the public legal queries and SafeHtml", () => {
  const page = source("../../pages/LegalPage.vue");
  assert.match(page, /from "@\/lib\/queries\/legal"/);
  assert.match(page, /legalDocQuery/);
  assert.match(page, /legalVersionsQuery/);
  assert.match(page, /asSafeHtml/);
  assert.match(page, /SafeHtml/);
  assert.match(page, /status === 404/);
  assert.match(page, /t\(['"]legal.empty['"]\)/);
  assert.match(page, /`\/legal\/\$\{kind\}\?version=\$\{entry\.version\}`/);
  assert.doesNotMatch(page, /from ["']@\/lib\/queries\/admin["']/);
  assert.doesNotMatch(page, /RouterLink/);
  assert.doesNotMatch(page, /from ["']react["']/);
  assert.doesNotMatch(page, /from ["']@fvoci\/editor\/safe-html["']/);
});

await test("the Vue service-info page fails once and retries the public instance", () => {
  const page = source("../../pages/ServiceInfoPage.vue");
  assert.match(page, /from "@\/lib\/queries\/instance"/);
  assert.match(page, /retry: false/);
  assert.match(page, /OperatorInfoView/);
  assert.doesNotMatch(page, /from ["']@\/lib\/queries\/admin["']/);
  assert.doesNotMatch(page, /RouterLink/);
});

await test("authenticated legal nav and operator view cross with plain anchors", () => {
  const nav = source("AuthenticatedLegalNav.vue");
  assert.match(nav, /href="\/service-info"/);
  assert.match(nav, /:href="`\/legal\/\$\{doc\.kind\}`"/);
  assert.doesNotMatch(nav, /RouterLink/);

  const view = source("OperatorInfoView.vue");
  assert.match(view, /operatorFieldHref/);
  assert.match(view, /hasOperatorInfo/);
  assert.match(view, /t\(['"]operator.empty['"]\)/);
  assert.doesNotMatch(view, /RouterLink/);
});
