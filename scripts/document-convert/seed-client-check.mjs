#!/usr/bin/env node
// Dev/test only. Opens Rust-seeded updates the way the editor client does:
// Yjs 13.6.32 applies the update, y-tiptap builds the ProseMirror root node
// with the editor schema, and the result must equal the TS seed's.
//   node --import <tsx> seed-client-check.mjs <collab-engine bin> <case.json>...
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { tiptapJsonToYDoc, yDocToTiptapJson } from "@fvoci/editor/collab-tiptap";
import { createFvociExtensions } from "@fvoci/editor/tiptap-schema";

const ROOT = dirname(fileURLToPath(import.meta.url));
// The ESM entry the editor package itself loads (one Yjs instance).
const esm = (pkg) => {
	const dir = join(ROOT, "../../packages/editor/node_modules", pkg);
	const entry = JSON.parse(readFileSync(join(dir, "package.json"), "utf8")).exports["."].import;
	return import(pathToFileURL(join(dir, entry)).href);
};
const Y = await esm("yjs");
const { yXmlFragmentToProseMirrorRootNode } = await esm("@tiptap/y-tiptap");
const { getSchema } = await esm("@tiptap/core");
const schema = getSchema(createFvociExtensions());

const [engine, ...files] = process.argv.slice(2);

function rustSeed(json) {
	const req = Buffer.from(JSON.stringify({ op: "seed_from_tiptap", content_json: JSON.stringify(json) }));
	const len = Buffer.alloc(4);
	len.writeUInt32LE(req.length);
	const out = spawnSync(engine, [], { input: Buffer.concat([len, req]), maxBuffer: 64 << 20 });
	const n = out.stdout.readUInt32LE(0);
	const report = JSON.parse(out.stdout.subarray(4, 4 + n).toString("utf8"));
	const outcome = report.outcome ?? report;
	if (outcome.status !== "ok") return { refused: outcome.status };
	return { update: Buffer.from(outcome.update_b64, "base64") };
}

// Mark arrays and object keys in a canonical order: Yrs encodes a mark's attrs
// map in hash order and y-tiptap keeps that order in raw JSON (ProseMirror
// rebuilds attrs in schema order, so `pm.toJSON()` is compared as is).
const canonical = (v) =>
	Array.isArray(v)
		? v.map(canonical)
		: v && typeof v === "object"
			? Object.fromEntries(
					Object.keys(v)
						.sort()
						.map((k) => {
							const c = canonical(v[k]);
							return [k, k === "marks" ? [...c].sort((a, b) => (JSON.stringify(a) < JSON.stringify(b) ? -1 : 1)) : c];
						}),
				)
			: v;
const sortMarks = (v) =>
	Array.isArray(v)
		? v.map(sortMarks)
		: v && typeof v === "object"
			? Object.fromEntries(
					Object.entries(v).map(([k, c]) => [
						k,
						k === "marks" ? [...c].sort((a, b) => (JSON.stringify(a) < JSON.stringify(b) ? -1 : 1)) : sortMarks(c),
					]),
				)
			: v;

function open(doc) {
	const pm = yXmlFragmentToProseMirrorRootNode(doc.getXmlFragment("prosemirror"), schema);
	let valid = true;
	try {
		pm.check();
	} catch {
		valid = false;
	}
	return { json: JSON.stringify(sortMarks(pm.toJSON())), valid, tiptap: JSON.stringify(canonical(yDocToTiptapJson(doc))) };
}

let ok = 0;
let refused = 0;
let failed = 0;
for (const file of files) {
	const input = JSON.parse(readFileSync(file, "utf8"));
	let oracle;
	try {
		oracle = open(tiptapJsonToYDoc(input));
	} catch {
		oracle = null;
	}
	const rust = rustSeed(input);
	if (!oracle || rust.refused) {
		if (!oracle && rust.refused) refused++;
		else {
			failed++;
			console.error(`MISMATCH ${file}: oracle ${oracle ? "ok" : "throws"}, rust ${rust.refused ?? "ok"}`);
		}
		continue;
	}
	const doc = new Y.Doc({ gc: false });
	Y.applyUpdate(doc, rust.update);
	const got = open(doc);
	const diff = ["json", "valid", "tiptap"].filter((k) => got[k] !== oracle[k]);
	if (diff.length === 0) ok++;
	else {
		failed++;
		console.error(`MISMATCH ${file}: ${diff.join(",")}`);
		if (process.env.SEED_CHECK_VERBOSE) for (const k of diff) console.error(` rust   ${got[k]}\n oracle ${oracle[k]}`);
	}
}
console.log(JSON.stringify({ ok, refused, failed }));
process.exit(failed ? 1 : 0);
