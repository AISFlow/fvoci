import assert from "node:assert/strict";
import test from "node:test";
import { isOidcConfigured, workspaceOidcForm } from "./workspace-oidc.ts";

test("workspaceOidcForm accepts an http issuer and required credentials", () => {
  const parsed = workspaceOidcForm.safeParse({
    issuer: "https://idp.example/realms/acme",
    clientId: "fvoci",
    clientSecret: "s",
    label: "Acme IdP",
  });
  assert.equal(parsed.success, true);
});

test("workspaceOidcForm refuses a non-http issuer", () => {
  const parsed = workspaceOidcForm.safeParse({
    issuer: "not-a-url",
    clientId: "fvoci",
    clientSecret: "s",
    label: "",
  });
  assert.equal(parsed.success, false);
});

test("isOidcConfigured requires issuer, client, and label", () => {
  assert.equal(isOidcConfigured(null), false);
  assert.equal(
    isOidcConfigured({
      issuer: "https://idp.example",
      clientId: "id",
      label: "IdP",
      redirectUri: "https://app.example/callback",
    }),
    true,
  );
  assert.equal(
    isOidcConfigured({
      issuer: "https://idp.example",
      clientId: "id",
      label: null,
      redirectUri: "https://app.example/callback",
    }),
    false,
  );
});
