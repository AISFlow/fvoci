import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { compileScript, compileTemplate, parse } from "@vue/compiler-sfc";
import * as VueQuery from "@tanstack/vue-query";
import { onlineManager } from "@tanstack/query-core";
import ts from "typescript";
import * as Vue from "vue";
import { t } from "@fvoci/i18n";
import { ProblemError } from "@/lib/api";
import * as queries from "@/lib/queries/share";
import { failMessage } from "./public-share-fail";

// Compile the actual page's script AND template. Only transport, routing and
// leaf presentation are substituted; Vue, its renderer and VueQuery are real.
type Node = { type: string; text: string; props: Record<string, any>; children: Node[]; parent: Node | null };
function node(type: string, text = ""): Node {
  return { type, text, props: {}, children: [], parent: null };
}
const renderer = Vue.createRenderer<Node, Node>({
  createElement: (type) => node(type),
  createText: (text) => node("text", text),
  createComment: (text) => node("comment", text),
  setText: (el, text) => { el.text = text; },
  setElementText: (el, text) => { el.text = text; el.children = []; },
  patchProp: (el, key, _old, value) => { el.props[key] = value; },
  parentNode: (el) => el.parent,
  nextSibling: (el) => el.parent?.children[el.parent.children.indexOf(el) + 1] ?? null,
  insert: (el, parent, anchor) => {
    if (el.parent) el.parent.children.splice(el.parent.children.indexOf(el), 1);
    const index = anchor ? parent.children.indexOf(anchor) : -1;
    parent.children.splice(index < 0 ? parent.children.length : index, 0, el);
    el.parent = parent;
  },
  remove: (el) => {
    el.parent?.children.splice(el.parent.children.indexOf(el), 1);
    el.parent = null;
  },
});

function evaluate(code: string, imports: Record<string, Record<string, unknown>>, result: string): any {
  const ast = ts.createSourceFile("page.ts", code, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
  const bindings: Record<string, unknown> = {};
  for (const statement of [...ast.statements].reverse()) {
    if (!ts.isImportDeclaration(statement)) continue;
    const module = imports[(statement.moduleSpecifier as ts.StringLiteral).text];
    const clause = statement.importClause;
    if (clause?.name) bindings[clause.name.text] = module.default;
    if (clause?.namedBindings && ts.isNamedImports(clause.namedBindings)) {
      for (const binding of clause.namedBindings.elements) {
        bindings[binding.name.text] = module[binding.propertyName?.text ?? binding.name.text];
      }
    }
    code = code.slice(0, statement.getFullStart()) + code.slice(statement.end);
  }
  const js = new Bun.Transpiler({ loader: "ts" }).transformSync(code.replace(/export default/, "return").replace(/export function render/, "function render"));
  return new Function(...Object.keys(bindings), `${js}\n${result}`)(...Object.values(bindings));
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

async function until(condition: () => boolean, what: string): Promise<void> {
  for (let i = 0; i < 200; i += 1) {
    await Vue.nextTick();
    if (condition()) return;
    await new Promise(resolve => setTimeout(resolve, 5));
  }
  assert.fail(`timed out waiting for ${what}`);
}

function descendants(root: Node): Node[] {
  return [root, ...root.children.flatMap(descendants)];
}

function mountPage() {
  const route = Vue.reactive({ params: { token: "share-a" } });
  const client = new VueQuery.QueryClient({ defaultOptions: { queries: { retry: false, gcTime: Infinity } } });
  // Bun isServer skips the browser plugin lifecycle; mount reconnect listeners
  // explicitly, just as VueQueryPlugin does in the actual browser.
  client.mount();
  const requests: { token: string; kind: string; documentId?: string | null }[] = [];
  const pending: Record<string, ReturnType<typeof deferred<any>>[]> = {};
  let readerMounts = 0;
  const request = (kind: string, token: string, documentId?: string | null) => {
    requests.push({ kind, token, documentId });
    const key = `${token}:${kind}`;
    const controlled = pending[key]?.[0];
    if (controlled) {
      const release = () => { if (pending[key]?.[0] === controlled) pending[key].shift(); };
      void controlled.promise.then(release, release);
      return controlled.promise;
    }
    if (documentId === "child") return Promise.reject(new ProblemError(404));
    return Promise.resolve(kind === "meta"
      ? { documentId: "root", title: `cached title ${token}`, expiresAt: null }
      : kind === "tree" ? { items: [{ id: "root", title: `cached tree ${token}` }] }
      : `<p>cached body ${token}</p>`);
  };
  const controlledQueries = {
    sharePublicMetaQuery: (token: string) => ({ ...queries.sharePublicMetaQuery(token), queryFn: () => request("meta", token) }),
    sharePublicTreeQuery: (token: string, enabled: boolean) => ({ ...queries.sharePublicTreeQuery(token, enabled), queryFn: () => request("tree", token) }),
    sharePublicBodyQuery: (token: string, id: string | null, enabled: boolean) => ({ ...queries.sharePublicBodyQuery(token, id, enabled), queryFn: () => request("body", token, id) }),
  };
  const reader = Vue.defineComponent({
    setup(_props, { attrs }) {
      readerMounts += 1;
      return () => Vue.h("reader", attrs, String(attrs.title));
    },
  });
  const button = Vue.defineComponent({ setup: (_props, { attrs, slots }) => () => Vue.h("button", attrs, slots.default?.()) });
  const filename = new URL("../../pages/PublicSharePage.vue", import.meta.url).pathname;
  const { descriptor } = parse(readFileSync(filename, "utf8"));
  const script = compileScript(descriptor, { id: "recovery-test" });
  const imports = {
    vue: Vue,
    "@fvoci/i18n": { t },
    "@tanstack/vue-query": VueQuery,
    "vue-router": { useRoute: () => route },
    "@/lib/api": { ProblemError },
    "@/lib/queries/share": controlledQueries,
    "@nuxt/ui/components/Button.vue": { default: button },
    "../features/share/PublicShareView.vue": { default: reader },
    "../features/share/public-share-fail": { failMessage },
  };
  const component = evaluate(script.content, imports, "");
  const template = compileTemplate({ source: descriptor.template!.content, filename, id: "recovery-test", compilerOptions: { bindingMetadata: script.bindings } });
  assert.deepEqual(template.errors, []);
  component.render = evaluate(template.code, imports, "return render;");
  const root = node("root");
  const app = renderer.createApp(component);
  app.use(VueQuery.VueQueryPlugin, { queryClient: client });
  app.mount(root);
  return {
    client, route, requests,
    get readerMounts() { return readerMounts; },
    reader: () => descendants(root).find(el => el.type === "reader"),
    denied: () => descendants(root).some(el => el.props.role === "alert"),
    queue(kind: string, token = route.params.token) {
      const control = deferred<any>();
      (pending[`${token}:${kind}`] ??= []).push(control);
      return control;
    },
    refresh: () => descendants(root).find(el => el.type === "button")!.props.onClick() as Promise<void>,
    stop: () => { app.unmount(); client.unmount(); client.clear(); },
  };
}

async function denyChild(page: ReturnType<typeof mountPage>) {
  await until(() => page.reader()?.props.body?.includes("cached body"), "the authorized reader");
  page.reader()!.props.onSelectDocument("child");
  await until(page.denied, "the child denial gate");
  assert.equal(page.reader(), undefined);
}

test("recovery cannot reveal cached chrome after reconnect denies the root while the earlier tree is pending", async () => {
  onlineManager.setOnline(true);
  const page = mountPage();
  try {
    await denyChild(page);
    const tree = page.queue("tree");
    const body = page.queue("body");
    const recovery = page.refresh();
    await until(() => page.requests.some(r => r.kind === "body" && r.documentId === null) && page.client.getQueryState(["share-body", "share-a", null])?.fetchStatus === "fetching", "the recovery root request");
    body.resolve("<p>formerly authorized recovery body</p>");
    await until(() => page.client.getQueryState(["share-body", "share-a", null])?.fetchStatus === "idle", "the recovery body success");
    assert.equal(page.reader(), undefined, "tree still pending, gate stays closed");
    const newerBody = page.queue("body");
    const count = page.requests.length;
    onlineManager.setOnline(false);
    onlineManager.setOnline(true);
    await until(() => page.requests.slice(count).some(r => r.kind === "body"), "the reconnect root request");
    newerBody.reject(new ProblemError(404));
    await until(() => page.client.getQueryState(["share-body", "share-a", null])?.status === "error", "the newer root denial");
    const mountsAfterDenial = page.readerMounts;
    tree.resolve({ items: [{ id: "root", title: "formerly authorized tree" }] });
    await recovery;
    await Vue.nextTick();
    assert.equal((page.client.getQueryState(["share-body", "share-a", null])?.error as ProblemError).status, 404);
    console.log("reconnect recovery state", { currentBodyStatus: page.client.getQueryState(["share-body", "share-a", null])?.status, denied: page.denied(), readerVisible: Boolean(page.reader()), readerMountsAfterDenial: page.readerMounts - mountsAfterDenial });
    assert.equal(page.denied(), true, "the latest denial must survive older recovery completion");
    assert.equal(page.reader(), undefined, "no cached tree/title/body is rendered");
    assert.equal(page.readerMounts, mountsAfterDenial, "no transient authorized-content mount");
  } finally {
    page.stop();
    onlineManager.setOnline(true);
  }
});
