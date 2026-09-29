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

type AttrDump = {
  name: string;
  hasDefault: boolean;
  default: unknown;
  validate: string | null;
};
type NodeDump = { name: string; attrs: AttrDump[] };
type MarkDump = NodeDump & { rank: number; overlapping: boolean };
type SchemaDump = { nodes: NodeDump[]; marks: MarkDump[] };

type AttrSpec = { hasDefault: boolean; default: unknown; validate?: unknown };
// NodeType/MarkType fields that prosemirror-model's .d.ts leaves out.
type TypeInternals = { attrs: Readonly<Record<string, AttrSpec>>; rank: number };
const internals = (type: object) => type as unknown as TypeInternals;

// Same projection as scripts/document-convert/schema-dump.mjs, which writes
// the fixture the Rust seed tables are tested against.
function attrsOf(attrs: Readonly<Record<string, AttrSpec>>): AttrDump[] {
  return Object.entries(attrs).map(([name, a]) => ({
    name,
    hasDefault: a.hasDefault,
    default: a.default,
    validate: a.validate ? String(a.validate) : null,
  }));
}

function dumpSchema(schema: ReturnType<typeof getSchema>): SchemaDump {
  return {
    nodes: Object.entries(schema.nodes).map(([name, type]) => ({
      name,
      attrs: attrsOf(internals(type).attrs),
    })),
    marks: Object.entries(schema.marks).map(([name, type]) => ({
      name,
      rank: internals(type).rank,
      overlapping: !type.excludes(type),
      attrs: attrsOf(internals(type).attrs),
    })),
  };
}

// Never called: getSchema does not build views. Distinct functions let the
// wiring test check that each node gets its own entry.
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
  const fixture = JSON.parse(
    readFileSync(
      new URL("../../../compat/fixtures/yjs-seed/schema.json", import.meta.url),
      "utf8",
    ),
  ) as SchemaDump;
  const mention = fixture.nodes.find((node) => node.name === "mention");
  assert.ok(mention);
  // The fixture is getSchema(createFvociExtensions()). The editor has always
  // re-added Mention (with its label view) after that list, so mention is its
  // last node. The server looks nodes up by name and ranks marks by order
  // (crates/collab-engine/src/seed.rs); the node order is pinned here so the
  // editor's own order only changes on purpose.
  const expected: SchemaDump = {
    nodes: [...fixture.nodes.filter((node) => node !== mention), mention],
    marks: fixture.marks,
  };
  assert.deepEqual(dumpSchema(getSchema(extensions())), expected);
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
  for (const root of ["editor-extensions.ts", "entities.ts", "math-ml.ts"]) {
    walk(new URL(`../src/${root}`, import.meta.url));
  }
  assert.deepEqual(
    [...bare].filter((spec) => framework.test(spec)),
    [],
  );
});
