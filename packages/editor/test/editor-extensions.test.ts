import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import test from "node:test";
import { getSchema } from "@tiptap/core";
import * as Y from "yjs";
import {
  createFvociEditorExtensions,
  type FvociNodeViewName,
  type FvociNodeViews,
} from "../src/editor-extensions.ts";
import { Attachment } from "../src/nodes/attachment.ts";
import { Embed } from "../src/nodes/embed.ts";
import { MathBlock, MathInline } from "../src/nodes/math.ts";
import { Mermaid } from "../src/nodes/mermaid.ts";
import { dumpSchema, editorSchemaFixture } from "./schema-dump.ts";

// Never called: getSchema does not build views. Distinct functions let the
// wiring test check that each node gets its own entry. The addNodeView-only
// test below keeps any host map schema-neutral. The Vue host's real map is
// tested in vue-node-views.test.ts,
// and apps/web/e2e/workspace-wiki-flow.spec.ts checks the schema of the editor
// the web app mounts.
const stubView = () => () => ({ dom: {} as HTMLElement });
const nodeViews: FvociNodeViews = {
  mermaid: stubView,
  math: () => stubView(),
  mathInline: () => stubView(),
  embed: () => stubView(),
  attachment: () => stubView(),
};

function extensions() {
  return createFvociEditorExtensions({
    ydoc: new Y.Doc({ gc: false }),
    nodeViews,
    mentionItems: () => undefined,
    entityResolver: () => null,
    workspaceSlug: () => null,
    uploads: { anchors: new Map(), queue: () => {} },
  });
}

test("editor schema matches the server's yjs seed schema contract", () => {
  assert.deepEqual(dumpSchema(getSchema(extensions())), editorSchemaFixture());
});

test("host node views are applied only as addNodeView", () => {
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
    const { addNodeView: _baseView, ...baseConfig } = {
      ...base.config,
    } as Record<string, unknown>;
    assert.equal(addNodeView, nodeViews[name], name);
    assert.deepEqual(Object.keys(config).sort(), Object.keys(baseConfig).sort());
    for (const key of Object.keys(baseConfig)) {
      assert.equal(config[key], baseConfig[key], `${name}.${key}`);
    }
  }
});

test("the factory and its neutral modules import no UI framework", () => {
  const framework =
    /^(react|react-dom|vue|@tiptap\/react|@tiptap\/vue-3|@tiptap\/extension-drag-handle-react|@tiptap\/extension-drag-handle-vue-3|@hocuspocus\/provider-react|@radix-ui\/.*|@nuxt\/.*)(\/.*)?$/;
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
      // A .tsx module would pull in the React JSX runtime.
      const target = new URL(spec.replace(/\.js$/, ".ts"), url);
      assert.ok(existsSync(target), `${spec} imported by ${url.pathname}`);
      walk(target);
    }
  };
  for (const root of [
    "editor-extensions.ts",
    "entities.ts",
    "math-ml.ts",
    "embed-model.ts",
    "attachment-model.ts",
  ]) {
    walk(new URL(`../src/${root}`, import.meta.url));
  }
  assert.deepEqual(
    [...bare].filter((spec) => framework.test(spec)),
    [],
  );
});
