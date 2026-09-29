import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";

const dir = import.meta.dirname;

function source(file: string): string {
  return readFileSync(path.join(dir, file), "utf8");
}

test("the Vue SSO slug form submits through startWorkspaceSso, never to the server", () => {
  const page = source("SsoSlugForm.vue");
  assert.match(page, /<form\b/);
  assert.match(page, /event\.preventDefault\(\)/);
  assert.match(page, /startWorkspaceSso\(/);
  assert.doesNotMatch(page, /\b(method|action|formAction)=/);
});

test("the Vue login forms do not set method or action", () => {
  for (const file of ["LoginForm.vue", "EmailActionForm.vue", "SsoSlugForm.vue", "MfaStep.vue"]) {
    assert.doesNotMatch(source(file), /\b(method|action)=/, file);
  }
});

test("OIDC starts are plain anchors, not fetches", () => {
  const page = source("LoginForm.vue");
  assert.match(page, /oidcStartHref\(/);
  assert.match(page, /class="auth-shell__outline-link"/);
});

test("the login page leaves the Vue app with a full load", () => {
  const page = source("../../pages/LoginPage.vue");
  assert.match(page, /window\.location\.assign\(returnTo/);
  assert.match(page, /window\.location\.replace\(/);
  assert.match(page, /redirectTo\("\/setup"\)/);
  assert.match(page, /api\.POST\("\/api\/v1\/auth\/login"/);
  assert.match(page, /api\.POST\("\/api\/v1\/auth\/magic-link"/);
  assert.match(page, /api\.POST\("\/api\/v1\/auth\/password-reset"/);
  assert.match(page, /if \(setup\.isLoading\.value \|\| setup\.isError\.value\) return/);
  assert.match(page, /if \(mfaToken\.value === null\) mfaToken\.value = takeMfaFragment\(\)/);
  assert.match(page, /from "@\/lib\/queries\/instance"/);
  assert.doesNotMatch(page, /from ["']react["']/);
  assert.doesNotMatch(page, /from ["']@tanstack\/react-query["']/);
  assert.doesNotMatch(page, /from ["']@\/lib\/queries\/admin["']/);
});

test("the service-info footer crosses to React with plain anchors", () => {
  const page = source("ServiceInfoFooter.vue");
  assert.match(page, /href="\/service-info"/);
  assert.match(page, /:href="`\/legal\/\$\{doc\.kind\}`"/);
  assert.doesNotMatch(page, /RouterLink/);
});

test("auth fields are uncontrolled (no v-model / :value)", () => {
  const field = source("AuthField.vue");
  assert.doesNotMatch(field, /\bv-model\b/);
  assert.doesNotMatch(field, /:value=/);
  assert.match(field, /defineOptions\(\{\s*inheritAttrs:\s*false/);
});
