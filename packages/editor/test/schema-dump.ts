import { readFileSync } from "node:fs";
import type { getSchema } from "@tiptap/core";

// The schema projection of scripts/document-convert/schema-dump.mjs, which
// writes tests/fixtures/yjs-seed/schema.json (the table the Rust seed writer
// is tested against), for the editor-extension tests of each host.

export type AttrDump = {
  name: string;
  hasDefault: boolean;
  default: unknown;
  validate: string | null;
};
export type NodeDump = { name: string; attrs: AttrDump[] };
export type MarkDump = NodeDump & { rank: number; overlapping: boolean };
export type SchemaDump = { nodes: NodeDump[]; marks: MarkDump[] };
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

export function dumpSchema(schema: ReturnType<typeof getSchema>): SchemaDump {
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

/** The fixture as the editor orders it. The fixture is
 * getSchema(createFvociExtensions()); the editor has always re-added Mention
 * (with its label view) after that list, so mention is its last node. The
 * server looks nodes up by name and ranks marks by order
 * (crates/collab-engine/src/seed.rs); the node order is pinned so the
 * editor's own order only changes on purpose. */
export function editorSchemaFixture(): SchemaDump {
  const fixture = JSON.parse(
    readFileSync(
      new URL("../../../tests/fixtures/yjs-seed/schema.json", import.meta.url),
      "utf8",
    ),
  ) as SchemaDump;
  const mention = fixture.nodes.find((node) => node.name === "mention");
  if (!mention) throw new Error("fixture has no mention node");
  return {
    nodes: [...fixture.nodes.filter((node) => node !== mention), mention],
    marks: fixture.marks,
  };
}
