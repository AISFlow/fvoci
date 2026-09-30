import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { setupInput } from "@/lib/validators.ts";

const dir = import.meta.dirname;

function source(file: string): string {
  return readFileSync(path.join(dir, file), "utf8");
}

await test("the Vue setup form does not set method or action", () => {
  assert.doesNotMatch(source("SetupForm.vue"), /\b(method|action)=/);
});

await test("setup fields are the React form's ids, and family name is optional", () => {
  const form = source("SetupForm.vue");
  assert.match(form, /id="setup-family-name"/);
  assert.match(form, /id="setup-given-name"/);
  assert.match(form, /id="setup-email"/);
  assert.match(form, /id="setup-password"/);
  assert.match(form, /id="setup-workspace-name"/);
  assert.match(form, /id="setup-workspace-slug"/);
  assert.match(form, /autocomplete="family-name"/);
  assert.match(form, /autocomplete="given-name"/);
  assert.match(form, /autocomplete="new-password"/);
  assert.match(form, /placeholder="my-workspace"/);
  assert.match(form, /schema: setupInput/);
  assert.match(form, /submitSetup/);
  assert.match(form, /grid-cols-\[6rem_minmax\(0,1fr\)\]/);
  assert.doesNotMatch(form, /\bv-model\b/);
  assert.doesNotMatch(form, /from ["']react["']/);
});

await test("the setup page posts the admin account, then loads home with fresh queries", () => {
  const page = source("../../pages/SetupPage.vue");
  assert.match(page, /api\.POST\("\/api\/v1\/setup"/);
  assert.match(page, /familyName: input\.familyName \|\| undefined/);
  assert.match(page, /queryClient\.invalidateQueries\(\)/);
  assert.match(page, /window\.location\.replace\("\/"\)/);
  assert.match(page, /router\.replace\("\/login"\)/);
  assert.match(page, /from "@\/lib\/queries"/);
  assert.doesNotMatch(page, /from ["']react["']/);
  assert.doesNotMatch(page, /from ["']@tanstack\/react-query["']/);
  assert.doesNotMatch(page, /redirectTo\("\/login"\)/);
});

await test("setupInput accepts the first-instance admin form the React page posts", () => {
  const parsed = setupInput.parse({
    email: "  Admin@Example.COM  ",
    password: "supersecret1",
    givenName: " 관리자 ",
    familyName: "",
    workspaceName: " Vue Setup ",
    workspaceSlug: "vsetup",
  });
  assert.equal(parsed.email, "Admin@Example.COM");
  assert.equal(parsed.givenName, "관리자");
  assert.equal(parsed.familyName, "");
  assert.equal(parsed.workspaceName, "Vue Setup");
  assert.equal(parsed.workspaceSlug, "vsetup");
});

await test("setupInput refuses a short password and a non-slug workspace address", () => {
  const short = setupInput.safeParse({
    email: "admin@example.com",
    password: "short",
    givenName: "관리자",
    familyName: "",
    workspaceName: "Acme",
    workspaceSlug: "acme",
  });
  assert.equal(short.success, false);

  const slug = setupInput.safeParse({
    email: "admin@example.com",
    password: "supersecret1",
    givenName: "관리자",
    familyName: "",
    workspaceName: "Acme",
    workspaceSlug: "Acme",
  });
  assert.equal(slug.success, false);
});
