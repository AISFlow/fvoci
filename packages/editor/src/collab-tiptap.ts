import { getSchema } from "@tiptap/core";
import {
	prosemirrorJSONToYXmlFragment,
	yDocToProsemirrorJSON,
	ySyncPluginKey,
} from "@tiptap/y-tiptap";
import * as Y from "yjs";
import { FVOCI_YDOC_FRAGMENT } from "./collab/constants.js";
import { isTiptapDoc, type TiptapDoc } from "./json.js";
import { createFvociExtensions } from "./tiptap-schema.js";

export { isTiptapDoc, type TiptapDoc, ySyncPluginKey };

function isRecord(v: unknown): v is Record<string, unknown> {
	return typeof v === "object" && v !== null;
}

/** WHY: 비교 전용 ychange 는 저장 JSON 에 남기지 않는다. */
function withoutYChange(value: unknown): unknown {
	if (Array.isArray(value)) return value.map(withoutYChange);
	if (!isRecord(value)) return value;
	const out: Record<string, unknown> = {};
	for (const [key, child] of Object.entries(value)) {
		if (key === "ychange") continue;
		if (key === "marks" && Array.isArray(child)) {
			out.marks = child.filter(
				(mark) => !(isRecord(mark) && mark.type === "ychange"),
			);
			continue;
		}
		out[key] = withoutYChange(child);
	}
	return out;
}

const schema = getSchema(createFvociExtensions());

export function tiptapJsonToYDoc(
	json: TiptapDoc,
	fragment = FVOCI_YDOC_FRAGMENT,
): Y.Doc {
	const doc = new Y.Doc({ gc: false });
	const payload = { type: "doc" as const, content: json.content ?? [] };
	prosemirrorJSONToYXmlFragment(schema, payload, doc.getXmlFragment(fragment));
	return doc;
}

export function yDocToTiptapJson(
	doc: Y.Doc,
	fragment = FVOCI_YDOC_FRAGMENT,
): TiptapDoc {
	const json = withoutYChange(yDocToProsemirrorJSON(doc, fragment));
	if (isTiptapDoc(json)) return json;
	return { type: "doc", content: [] };
}

export function replaceYDocContent(
	doc: Y.Doc,
	json: TiptapDoc,
	fragment = FVOCI_YDOC_FRAGMENT,
): void {
	const frag = doc.getXmlFragment(fragment);
	const payload = { type: "doc" as const, content: json.content ?? [] };
	doc.transact(() => {
		if (frag.length > 0) frag.delete(0, frag.length);
		prosemirrorJSONToYXmlFragment(schema, payload, frag);
	});
}
