import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";

const dir = import.meta.dirname;

function source(file: string): string {
  return readFileSync(path.join(dir, file), "utf8");
}

await test("the Vue invite form does not set method or action", () => {
  assert.doesNotMatch(source("InviteAcceptForm.vue"), /\b(method|action|formAction|formMethod)=/);
});

await test("the Vue invite page starts OIDC by script, never by a native form", () => {
  const page = source("InviteAcceptForm.vue");
  assert.match(page, /clickOidcStart\([^;]*?\bstartOidcInvite\(/);
  assert.doesNotMatch(page, /\b(method|action|formAction|formMethod)=/);
  assert.doesNotMatch(page, /oidcStartHref\(/);
  assert.doesNotMatch(page, /<form\b[^>]*(method|action)=/);
});

await test("the invite page leaves the Vue app with a full load", () => {
  const page = source("../../pages/InvitePage.vue");
  assert.match(page, /window\.location\.assign\("\/"\)/);
  assert.match(page, /window\.location\.assign\("\/login"\)/);
  assert.match(page, /redirectTo\("\/setup"\)/);
  assert.match(page, /api\.POST\("\/api\/v1\/invitations\/\{token\}\/accept"/);
  assert.match(page, /invitationPublicQuery/);
  assert.match(page, /enabled: setupReady\.value/);
  assert.doesNotMatch(page, /from ["']react["']/);
  assert.doesNotMatch(page, /from ["']@tanstack\/react-query["']/);
  assert.doesNotMatch(page, /takeMfaFragment/);
});

await test("invite consents and legal links are not Vue-router navigations", () => {
  const page = source("InviteAcceptForm.vue");
  assert.match(page, /requiredLegal/);
  assert.match(page, /:href="`\/legal\/\$\{doc\.kind\}`"/);
  assert.match(page, /target="_blank"/);
  assert.doesNotMatch(page, /RouterLink/);
});
