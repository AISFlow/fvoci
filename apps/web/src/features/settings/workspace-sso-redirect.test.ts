import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { compileScript, compileTemplate, parse } from "@vue/compiler-sfc";
import { renderToString } from "vue/server-renderer";
import { evaluate, compiledComponent, renderFunction, callCopy } from "./compiled-component-test";
import * as Vue from "vue";
import { t } from "@fvoci/i18n";
import * as api from "@/lib/api";
import * as form from "../../vue/features/settings/form.ts";
import * as oidcForm from "../../vue/features/settings/workspace-oidc.ts";
import { copyText } from "../../vue/features/settings/clipboard.ts";
import {
  displayedRedirectUri,
  workspaceSsoRedirectUri,
} from "../../vue/features/settings/sso-uri.ts";

const WORKSPACE_ID = "01900000-0000-7000-8000-000000000001";
const filename = new URL("../../vue/features/settings/WorkspaceSsoSection.vue", import.meta.url)
  .pathname;
const { descriptor } = parse(readFileSync(filename, "utf8"), { filename });
const script = compileScript(descriptor, { id: "sso-contract" });
assert.ok(descriptor.template, "actual component has a template");
const template = compileTemplate({
  source: descriptor.template.content,
  filename,
  id: "sso-contract",
  compilerOptions: { bindingMetadata: script.bindings },
});
assert.deepEqual(template.errors, []);

// Compile the actual Vue script and template; substitute query snapshots and
// leaf controls only. Policy computations, event handlers and rendering are real.

const button = Vue.defineComponent({
  setup:
    (_props, { attrs, slots }) =>
    () =>
      Vue.h("button", attrs, slots.default?.()),
});
const input = Vue.defineComponent({
  props: ["modelValue"],
  setup:
    (props, { attrs }) =>
    () =>
      Vue.h("input", { ...attrs, value: props.modelValue }),
});

async function render(copy?: "copied" | "failed") {
  const originalWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
  const originalNavigator = Object.getOwnPropertyDescriptor(globalThis, "navigator");
  const written: string[] = [];
  try {
    Object.defineProperty(globalThis, "window", {
      configurable: true,
      value: { location: { origin: "http://intranet:8080" } },
    });
    Object.defineProperty(globalThis, "navigator", {
      configurable: true,
      value:
        copy === "failed"
          ? {}
          : {
              clipboard: {
                writeText: (value: string) => {
                  written.push(value);
                  return Promise.resolve();
                },
              },
            },
    });
    const idle = () => ({ isPending: Vue.ref(false) });
    const imports = {
      vue: Vue,
      "@fvoci/i18n": { t },
      "@/lib/api": api,
      "@tanstack/vue-query": {
        useQuery: () => ({
          data: Vue.ref({
            redirectUri: workspaceSsoRedirectUri("https://fvoci.example", WORKSPACE_ID),
          }),
          isLoading: Vue.ref(false),
          isError: Vue.ref(false),
          error: Vue.ref(null),
        }),
        useMutation: idle,
        useQueryClient: () => ({}),
      },
      "@nuxt/ui/components/Button.vue": { default: button },
      "@nuxt/ui/components/Input.vue": { default: input },
      "./ConfirmAction.vue": { default: button },
      "./clipboard": { copyText },
      "./form": form,
      "./sso-uri": { displayedRedirectUri },
      "./workspace-oidc": oidcForm,
      "@/features/settings/settings-shell.css": {},
    };
    const component = compiledComponent(evaluate(script.content, imports).default);
    component.render = renderFunction(evaluate(template.code, imports).render);
    const setup = component.setup;
    component.setup = (props: Record<string, unknown>, context: Vue.SetupContext) => {
      const state = setup(props, context);
      if (copy) Vue.onServerPrefetch(() => callCopy(state));
      return state;
    };
    const html = await renderToString(Vue.createSSRApp(component, { workspaceId: WORKSPACE_ID }));
    if (copy === "copied")
      assert.deepEqual(written, [workspaceSsoRedirectUri("https://fvoci.example", WORKSPACE_ID)]);
    if (copy === "failed") assert.deepEqual(written, []);
    return html;
  } finally {
    for (const [key, descriptor] of [
      ["window", originalWindow],
      ["navigator", originalNavigator],
    ] as const) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else Reflect.deleteProperty(globalThis, key);
    }
  }
}

function escape(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

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
  // An admin on another host name still sees the public-origin URI.
  assert.equal(displayedRedirectUri(server, "http://intranet:8080", WORKSPACE_ID), server);
  for (const missing of [undefined, null, ""]) {
    assert.equal(
      displayedRedirectUri(missing, "http://intranet:8080", WORKSPACE_ID),
      `http://intranet:8080/api/v1/auth/sso/${WORKSPACE_ID}/callback`,
    );
  }
});

await test("the Vue section shows the server URI read-only with Korean help and a copy button", async () => {
  const html = await render();
  assert.match(html, /<label[^>]*>리디렉션 URI<\/label>/);
  const input = html.match(/<input[^>]*>/)?.[0] ?? "";
  assert.ok(
    input.includes(`value="https://fvoci.example/api/v1/auth/sso/${WORKSPACE_ID}/callback"`),
    input,
  );
  assert.match(input, /readonly(?:=|\s|>)/);
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

await test("the actual Vue copy handler sets copied text or an alert to copy by hand", async () => {
  assert.match(await render("copied"), /<button type="button"[^>]*>복사됨<\/button>/);
  const failed = await render("failed");
  assert.match(failed, /<button type="button"[^>]*>복사<\/button>/);
  assert.match(
    failed,
    /<p role="alert"[^>]*>주소를 복사하지 못했습니다\. 입력란에서 직접 선택해 복사해 주세요\.<\/p>/,
  );
});

await test("copyText writes to the clipboard and fails without one", async () => {
  const original = Object.getOwnPropertyDescriptor(globalThis, "navigator");
  const written: string[] = [];
  try {
    Object.defineProperty(globalThis, "navigator", {
      configurable: true,
      value: {
        clipboard: {
          writeText: (value: string) => {
            written.push(value);
            return Promise.resolve();
          },
        },
      },
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
