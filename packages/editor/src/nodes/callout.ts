import { mergeAttributes, Node } from "@tiptap/core";

export const CALLOUT_KINDS = ["note", "tip", "warning", "caution"] as const;
export type CalloutKind = (typeof CALLOUT_KINDS)[number];

export function isCalloutKind(value: unknown): value is CalloutKind {
	return (
		typeof value === "string" &&
		(CALLOUT_KINDS as readonly string[]).includes(value)
	);
}

export const Callout = Node.create({
	name: "callout",
	group: "block",
	content: "block+",
	defining: true,
	addAttributes() {
		return {
			kind: {
				default: "note" satisfies CalloutKind,
				parseHTML: (el) => {
					const v = el.getAttribute("data-kind");
					return isCalloutKind(v) ? v : "note";
				},
				renderHTML: (attributes) => ({
					"data-kind": isCalloutKind(attributes.kind)
						? attributes.kind
						: "note",
				}),
			},
		};
	},
	parseHTML() {
		return [{ tag: "aside[data-callout]" }];
	},
	renderHTML({ HTMLAttributes }) {
		return [
			"aside",
			mergeAttributes(HTMLAttributes, {
				"data-callout": "",
				class: "afn-callout",
			}),
			0,
		];
	},
});
