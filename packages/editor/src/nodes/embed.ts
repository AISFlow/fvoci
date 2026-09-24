import { mergeAttributes, Node } from "@tiptap/core";

export const Embed = Node.create({
	name: "embed",
	group: "block",
	atom: true,
	addAttributes() {
		return {
			entity: { default: "document" },
			ref: { default: "" },
		};
	},
	parseHTML() {
		return [{ tag: "div[data-embed]" }];
	},
	renderHTML({ HTMLAttributes }) {
		return ["div", mergeAttributes(HTMLAttributes, { "data-embed": "" })];
	},
});
