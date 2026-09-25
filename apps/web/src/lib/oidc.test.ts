import assert from "node:assert/strict";
import test from "node:test";
import { t } from "@fvoci/i18n";
import {
  OIDC_ERROR_CODES,
  oidcErrorMessage,
  oidcLinkAction,
  oidcStartHref,
  readMfaFragment,
} from "./oidc.ts";

test("oidcErrorMessage maps each callback code to its catalog message", () => {
  for (const code of OIDC_ERROR_CODES) {
    const message = oidcErrorMessage(code);
    assert.equal(message, t(code));
    assert.notEqual(message, t("oidc_fallback"));
  }
  assert.equal(oidcErrorMessage("oidc_last_method"), "마지막 로그인 수단은 해제할 수 없습니다.");
});

test("oidcErrorMessage falls back for unknown codes and stays silent without one", () => {
  assert.equal(oidcErrorMessage("something_else"), t("oidc_fallback"));
  assert.equal(oidcErrorMessage("__proto__"), t("oidc_fallback"));
  assert.equal(oidcErrorMessage(null), null);
  assert.equal(oidcErrorMessage(undefined), null);
  assert.equal(oidcErrorMessage(""), null);
});

test("readMfaFragment returns the pending token from #mfa=", () => {
  assert.equal(readMfaFragment("#mfa=abc_DEF-123"), "abc_DEF-123");
  assert.equal(readMfaFragment("mfa=abc"), "abc");
  assert.equal(readMfaFragment("#other=1&mfa=tok%2Bx"), "tok+x");
});

test("readMfaFragment ignores empty or unrelated fragments", () => {
  assert.equal(readMfaFragment(""), null);
  assert.equal(readMfaFragment("#"), null);
  assert.equal(readMfaFragment("#mfa="), null);
  assert.equal(readMfaFragment("#section-2"), null);
});

test("oidcStartHref carries the invitation token and consents", () => {
  assert.equal(oidcStartHref("google"), "/api/v1/auth/oidc/google/start");
  const href = oidcStartHref("google", {
    token: "inv123",
    consents: [{ kind: "terms", version: 2 }],
  });
  const url = new URL(href, "https://fvoci.example");
  assert.equal(url.pathname, "/api/v1/auth/oidc/google/start");
  assert.equal(url.searchParams.get("invitation"), "inv123");
  assert.deepEqual(JSON.parse(url.searchParams.get("consents") ?? ""), [
    { kind: "terms", version: 2 },
  ]);
  assert.equal(oidcLinkAction("a/b"), "/api/v1/auth/oidc/a%2Fb/link");
});
