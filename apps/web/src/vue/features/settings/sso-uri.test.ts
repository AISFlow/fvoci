import assert from "node:assert/strict";
import test from "node:test";
import { displayedRedirectUri, workspaceSsoRedirectUri } from "./sso-uri.ts";

const WORKSPACE_ID = "01900000-0000-7000-8000-000000000001";

await test("redirect URI is the per-workspace SSO callback on the public origin", () => {
  const expected = `https://fvoci.example/api/v1/auth/sso/${WORKSPACE_ID}/callback`;
  assert.equal(workspaceSsoRedirectUri("https://fvoci.example", WORKSPACE_ID), expected);
  assert.equal(workspaceSsoRedirectUri("https://fvoci.example/", WORKSPACE_ID), expected);
  assert.equal(
    workspaceSsoRedirectUri("http://localhost:5173", "a/b"),
    "http://localhost:5173/api/v1/auth/sso/a%2Fb/callback",
  );
});

await test("the shown URI is the server's; the browser origin is only a fallback", () => {
  const server = `https://fvoci.example/api/v1/auth/sso/${WORKSPACE_ID}/callback`;
  assert.equal(displayedRedirectUri(server, "http://intranet:8080", WORKSPACE_ID), server);
  for (const missing of [undefined, null, ""]) {
    assert.equal(
      displayedRedirectUri(missing, "http://intranet:8080", WORKSPACE_ID),
      `http://intranet:8080/api/v1/auth/sso/${WORKSPACE_ID}/callback`,
    );
  }
});
