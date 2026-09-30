import assert from "node:assert/strict";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import test from "node:test";
import { getSchema } from "@tiptap/core";
import * as Y from "yjs";
import { createFvociEditorExtensions, type FvociNodeViewName } from "../src/editor-extensions.ts";
import { Attachment } from "../src/nodes/attachment.ts";
import { Embed } from "../src/nodes/embed.ts";
import { MathBlock, MathInline } from "../src/nodes/math.ts";
import { Mermaid } from "../src/nodes/mermaid.ts";
import { VUE_NODE_VIEWS } from "../src/vue/node-views.ts";
import { dumpSchema, editorSchemaFixture } from "./schema-dump.ts";

// The Vue host's real node-view map (its .vue modules compiled by
// test/setup/vue-sfc.ts), through the shared extension factory.
function extensions() {
  return createFvociEditorExtensions({
    ydoc: new Y.Doc({ gc: false }),
    nodeViews: VUE_NODE_VIEWS,
    mentionItems: () => undefined,
    entityResolver: () => null,
    workspaceSlug: () => null,
    uploads: { anchors: new Map(), queue: () => {} },
  });
}

await test("the Vue node views give exactly the server's yjs seed schema", () => {
  assert.deepEqual(dumpSchema(getSchema(extensions())), editorSchemaFixture());
});

await test("the Vue node views change only addNodeView; mermaid keeps its plain view", () => {
  const list = extensions();
  const bases = {
    mermaid: Mermaid,
    math: MathBlock,
    mathInline: MathInline,
    embed: Embed,
    attachment: Attachment,
  } satisfies Record<FvociNodeViewName, unknown>;
  for (const [name, base] of Object.entries(bases) as [
    FvociNodeViewName,
    (typeof bases)[FvociNodeViewName],
  ][]) {
    const ext = list.find((candidate) => candidate.name === name);
    assert.ok(ext, name);
    assert.equal(ext.parent, base, name);
    const { addNodeView, ...config } = { ...ext.config } as Record<string, unknown>;
    const baseConfig: Record<string, unknown> = { ...base.config };
    delete baseConfig.addNodeView;
    assert.equal(addNodeView, VUE_NODE_VIEWS[name], name);
    assert.deepEqual(Object.keys(config).sort(), Object.keys(baseConfig).sort());
    for (const key of Object.keys(baseConfig)) {
      assert.equal(config[key], baseConfig[key], `${name}.${key}`);
    }
  }
  assert.equal(VUE_NODE_VIEWS.mermaid, undefined);
  const mermaid = list.find((candidate) => candidate.name === "mermaid");
  assert.equal(
    (mermaid?.config as { addNodeView?: unknown } | undefined)?.addNodeView,
    undefined,
    "falls back to Mermaid's own view",
  );
});

await test("the Vue editor imports no React module", () => {
  const react =
    /^(react|react-dom|@tiptap\/react|@tiptap\/extension-drag-handle-react|@hocuspocus\/provider-react|@radix-ui\/.*)(\/.*)?$/;
  const seen = new Set<string>();
  const bare = new Set<string>();
  const walk = (url: URL): void => {
    if (seen.has(url.href)) return;
    seen.add(url.href);
    const source = readFileSync(url, "utf8");
    for (const [, spec] of source.matchAll(/(?:from|import)\s*\(?\s*"([^"]+)"/g)) {
      if (!spec?.startsWith(".")) {
        if (spec) bare.add(spec);
        continue;
      }
      assert.equal(spec.endsWith(".tsx"), false, `${spec} imported by ${url.pathname}`);
      const target = new URL(spec.replace(/\.js$/, ".ts"), url);
      assert.ok(existsSync(target), `${spec} imported by ${url.pathname}`);
      walk(target);
    }
  };
  walk(new URL("../src/vue/index.ts", import.meta.url));
  assert.deepEqual(
    [...bare].filter((spec) => react.test(spec)),
    [],
  );
  assert.ok(bare.has("@tiptap/vue-3"));
});

// Vue re-applies a bound `value` whenever the template that owns the field
// re-renders, and a node view re-renders on a peer's change to its node: a
// bound field would drop what the user is typing. The fields are
// uncontrolled (the React views' defaultValue) and get their value once,
// when they open.
await test("the Vue node views never bind the value of a field being edited", () => {
  const dir = new URL("../src/vue/", import.meta.url);
  const bound: string[] = [];
  for (const name of readdirSync(dir).filter((file) => file.endsWith(".vue"))) {
    const source = readFileSync(new URL(name, dir), "utf8");
    for (const [tag] of source.matchAll(/<(?:input|textarea|select)\b[^>]*>/g)) {
      if (/(?:^|\s)(?::value|v-bind:value|v-model(?:\.[a-z]+)*)=/.test(tag))
        bound.push(`${name}: ${tag.replace(/\s+/g, " ")}`);
    }
  }
  assert.deepEqual(bound, []);
});
