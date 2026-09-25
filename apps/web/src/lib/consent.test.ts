import assert from "node:assert/strict";
import test from "node:test";
import { consentUrl, isConsentRequired, safeReturnTo } from "./consent.ts";

const origin = "https://fvoci.example";

test("safeReturnTo keeps same-origin paths and drops foreign or looping targets", () => {
  assert.equal(safeReturnTo("/w/acme/wiki?x=1#h", origin), "/w/acme/wiki?x=1#h");
  assert.equal(safeReturnTo(null, origin), "/");
  assert.equal(safeReturnTo("", origin), "/");
  assert.equal(safeReturnTo("//evil.example/x", origin), "/");
  assert.equal(safeReturnTo("https://evil.example/", origin), "/");
  assert.equal(safeReturnTo("javascript:alert(1)", origin), "/");
  assert.equal(safeReturnTo("/consent?returnTo=/x", origin), "/");
  assert.equal(safeReturnTo("/login", origin), "/");
});

test("consentUrl encodes the current location as returnTo", () => {
  assert.equal(
    consentUrl({ pathname: "/w/a b", search: "?q=1", hash: "#c" }),
    "/consent?returnTo=%2Fw%2Fa%20b%3Fq%3D1%23c",
  );
  assert.equal(consentUrl({ pathname: "/login", search: "", hash: "" }), "/consent?returnTo=%2F");
});

test("isConsentRequired only matches the 428 consent problem", () => {
  assert.equal(isConsentRequired(428, { code: "consent_required" }), true);
  assert.equal(isConsentRequired(428, { code: "other" }), false);
  assert.equal(isConsentRequired(403, { code: "consent_required" }), false);
  assert.equal(isConsentRequired(428, null), false);
});
