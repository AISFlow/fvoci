import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { t } from "@fvoci/i18n";
import {
  copyText,
  type RedirectCopyStatus,
  WorkspaceSsoRedirectUri,
  workspaceSsoRedirectUri,
} from "./workspace-sso-redirect.ts";

const WORKSPACE_ID = "01900000-0000-7000-8000-000000000001";

function escape(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function render(copyStatus: RedirectCopyStatus = null) {
  return renderToStaticMarkup(
    createElement(WorkspaceSsoRedirectUri, {
      uri: workspaceSsoRedirectUri("https://fvoci.example", WORKSPACE_ID),
      copyStatus,
      onCopy: () => {},
    }),
  );
}

test("redirect URI is the per-workspace SSO callback on the public origin", () => {
  const expected = `https://fvoci.example/api/v1/auth/sso/${WORKSPACE_ID}/callback`;
  assert.equal(workspaceSsoRedirectUri("https://fvoci.example", WORKSPACE_ID), expected);
  assert.equal(workspaceSsoRedirectUri("https://fvoci.example/", WORKSPACE_ID), expected);
  assert.equal(
    workspaceSsoRedirectUri("http://localhost:5173", "a/b"),
    "http://localhost:5173/api/v1/auth/sso/a%2Fb/callback",
  );
});

test("the section shows the URI read-only with Korean help and a copy button", () => {
  const html = render();
  assert.match(html, /<label[^>]*>리디렉션 URI<\/label>/);
  const input = html.match(/<input[^>]*>/)?.[0] ?? "";
  assert.ok(
    input.includes(`value="https://fvoci.example/api/v1/auth/sso/${WORKSPACE_ID}/callback"`),
    input,
  );
  assert.match(input, /readOnly=""/);
  const inputId = input.match(/id="([^"]+)"/)?.[1];
  const helpId = input.match(/aria-describedby="([^"]+)"/)?.[1];
  assert.ok(inputId && helpId);
  assert.match(html, new RegExp(`<label for="${escape(inputId)}"`));
  assert.match(
    html,
    new RegExp(`<p id="${escape(helpId)}"[^>]*>${escape(t("auth.sso.redirectUri.help"))}</p>`),
  );
  assert.match(t("auth.sso.redirectUri.help"), /ID 공급자\(IdP\)/);
  assert.match(html, /<button type="button"[^>]*>복사<\/button>/);
  assert.doesNotMatch(html, /role="alert"/);
});

test("copy outcome: copied label, or an alert to copy by hand", () => {
  assert.match(render("copied"), /<button type="button"[^>]*>복사됨<\/button>/);
  const failed = render("failed");
  assert.match(failed, /<button type="button"[^>]*>복사<\/button>/);
  assert.match(
    failed,
    /<p role="alert"[^>]*>주소를 복사하지 못했습니다\. 입력란에서 직접 선택해 복사해 주세요\.<\/p>/,
  );
});

test("copyText writes to the clipboard and fails without one", async () => {
  const original = Object.getOwnPropertyDescriptor(globalThis, "navigator");
  const written: string[] = [];
  try {
    Object.defineProperty(globalThis, "navigator", {
      configurable: true,
      value: { clipboard: { writeText: async (value: string) => void written.push(value) } },
    });
    await copyText("https://fvoci.example/api/v1/auth/sso/x/callback");
    assert.deepEqual(written, ["https://fvoci.example/api/v1/auth/sso/x/callback"]);
    Object.defineProperty(globalThis, "navigator", { configurable: true, value: {} });
    await assert.rejects(copyText("x"), /clipboard unavailable/);
  } finally {
    if (original) Object.defineProperty(globalThis, "navigator", original);
    else delete (globalThis as { navigator?: unknown }).navigator;
  }
});
