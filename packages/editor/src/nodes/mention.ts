import { mergeAttributes, Node } from "@tiptap/core";

export const Mention = Node.create({
	name: "mention",
	group: "inline",
	inline: true,
	atom: true,
	addAttributes() {
		return {
			entity: { default: "user" },
			id: { default: "" },
			label: { default: "" },
		};
	},
	parseHTML() {
		return [{ tag: "span[data-mention]" }];
	},
	renderHTML({ HTMLAttributes }) {
		const label =
			typeof HTMLAttributes.label === "string" ? HTMLAttributes.label : "";
		return [
			"span",
			mergeAttributes(HTMLAttributes, { "data-mention": "" }),
			`@${label}`,
		];
	},
});
