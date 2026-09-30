import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";

const dir = import.meta.dirname;

function source(file: string): string {
  return readFileSync(path.join(dir, file), "utf8");
}

await test("the Vue SSO slug form submits through startWorkspaceSso, never to the server", () => {
  const page = source("SsoSlugForm.vue");
  assert.match(page, /<form\b/);
  assert.match(page, /event\.preventDefault\(\)/);
  assert.match(page, /startWorkspaceSso\(/);
  assert.doesNotMatch(page, /\b(method|action|formAction)=/);
});

await test("the Vue login forms do not set method or action", () => {
  for (const file of [
    "LoginForm.vue",
    "EmailActionForm.vue",
    "SsoSlugForm.vue",
    "MfaStep.vue",
    "InviteAcceptForm.vue",
    "ResetPasswordView.vue",
    "MagicLinkView.vue",
    "ConfirmEmailView.vue",
    "CancelWithdrawView.vue",
    "ConsentView.vue",
  ]) {
    assert.doesNotMatch(source(file), /\b(method|action)=/, file);
  }
});

await test("OIDC starts are plain anchors, not fetches", () => {
  const page = source("LoginForm.vue");
  assert.match(page, /oidcStartHref\(/);
  assert.match(page, /class="auth-shell__outline-link"/);
});

await test("the login page leaves the Vue app with a full load", () => {
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

await test("the service-info footer opens public pages with plain anchors", () => {
  const page = source("ServiceInfoFooter.vue");
  assert.match(page, /href="\/service-info"/);
  assert.match(page, /:href="`\/legal\/\$\{doc\.kind\}`"/);
  assert.doesNotMatch(page, /RouterLink/);
});

await test("auth fields are uncontrolled (no v-model / :value)", () => {
  const field = source("AuthField.vue");
  assert.doesNotMatch(field, /\bv-model\b/);
  assert.doesNotMatch(field, /:value=/);
  assert.match(field, /defineOptions\(\{\s*inheritAttrs:\s*false/);
});

await test("the remaining auth pages call the React pages' APIs and do not consume on load", () => {
  const reset = source("../../pages/ResetPasswordPage.vue");
  assert.match(reset, /api\.POST\("\/api\/v1\/auth\/password-reset\/confirm"/);
  assert.match(reset, /router\.replace\("\/login\?reset=1"\)/);
  assert.match(reset, /redirectTo\("\/setup"\)/);
  assert.doesNotMatch(reset, /from ["']react["']/);

  const magic = source("../../pages/MagicLinkPage.vue");
  assert.match(magic, /api\.POST\("\/api\/v1\/auth\/magic-link\/consume"/);
  assert.match(magic, /window\.location\.replace\("\/"\)/);
  assert.match(source("MagicLinkView.vue"), /@click="handleClick"/);
  assert.doesNotMatch(source("MagicLinkView.vue"), /onMounted|watchEffect/);

  const confirm = source("../../pages/ConfirmEmailPage.vue");
  assert.match(confirm, /api\.POST\("\/api\/v1\/auth\/email\/confirm"/);
  assert.match(confirm, /window\.location\.replace\("\/settings\/account\?email_changed=1"\)/);
  assert.doesNotMatch(source("ConfirmEmailView.vue"), /onMounted|watchEffect/);

  const cancel = source("../../pages/CancelWithdrawPage.vue");
  assert.match(cancel, /parseErasureHash\(route\.hash\)/);
  assert.match(cancel, /api\.POST\("\/api\/v1\/auth\/cancel-withdraw"/);
  assert.match(cancel, /window\.history\.replaceState\(null, "", "\/cancel-withdraw"\)/);
  assert.doesNotMatch(source("CancelWithdrawView.vue"), /onMounted/);

  const consent = source("../../pages/ConsentPage.vue");
  assert.match(consent, /api\.GET\("\/api\/v1\/auth\/consents\/pending"\)/);
  assert.match(consent, /api\.POST\("\/api\/v1\/auth\/consents"/);
  assert.match(consent, /window\.location\.assign\(returnTo\.value\)/);
  assert.match(consent, /err\.status === 401/);
  assert.doesNotMatch(consent, /from ["']react["']/);
  assert.doesNotMatch(consent, /from ["']@tanstack\/react-query["']/);
});
