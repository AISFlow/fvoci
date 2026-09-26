#!/usr/bin/env node
// Dev/test only: prints the attrs (schema order, defaults) and mark
// overlap of getSchema(createFvociExtensions()) — the schema
// tiptapJsonToYUpdate seeds with. Read by compat/fixtures/yjs-seed tests.
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { createFvociExtensions } from "@fvoci/editor/tiptap-schema";

const ROOT = dirname(fileURLToPath(import.meta.url));
const require = createRequire(join(ROOT, "../../packages/editor/package.json"));
const { getSchema } = await import(require.resolve("@tiptap/core"));
const schema = getSchema(createFvociExtensions());
const attrs = (type) =>
	Object.entries(type.attrs).map(([name, a]) => ({
		name,
		hasDefault: a.hasDefault,
		default: a.default,
		validate: a.validate ? String(a.validate) : null,
	}));
const out = { nodes: [], marks: [] };
for (const [name, type] of Object.entries(schema.nodes)) out.nodes.push({ name, attrs: attrs(type) });
for (const [name, type] of Object.entries(schema.marks))
	out.marks.push({ name, rank: type.rank, overlapping: !type.excludes(type), attrs: attrs(type) });
process.stdout.write(JSON.stringify(out, null, 1) + "\n");
