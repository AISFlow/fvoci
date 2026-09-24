import { InputRule, mergeAttributes, Node } from "@tiptap/core";

export const Mermaid = Node.create({
	name: "mermaid",
	group: "block",
	atom: true,
	addAttributes() {
		return {
			source: { default: "" },
		};
	},
	parseHTML() {
		return [
			{
				tag: "[data-mermaid]",
				getAttrs: (el) => {
					if (!(el instanceof HTMLElement)) return false;
					return {
						source: el.getAttribute("data-source") ?? el.textContent ?? "",
					};
				},
			},
		];
	},
	renderHTML({ HTMLAttributes }) {
		const source =
			typeof HTMLAttributes.source === "string" ? HTMLAttributes.source : "";
		return [
			"pre",
			mergeAttributes(HTMLAttributes, {
				"data-mermaid": "",
				"data-source": source,
				class: "afn-mermaid-source",
			}),
			source,
		];
	},
	addInputRules() {
		return [
			new InputRule({
				find: /^```mermaid[\s\n]$/,
				handler: ({ chain, range }) => {
					chain()
						.deleteRange(range)
						.insertContent({ type: this.name, attrs: { source: "" } })
						.run();
				},
			}),
		];
	},
	addNodeView() {
		return ({ node }) => {
			const dom = document.createElement("pre");
			dom.setAttribute("data-mermaid", "");
			dom.className = "afn-mermaid-source";
			const sync = (n: typeof node) => {
				const source = typeof n.attrs.source === "string" ? n.attrs.source : "";
				dom.setAttribute("data-source", source);
				dom.textContent = source;
			};
			sync(node);
			return {
				dom,
				update(updated) {
					if (updated.type.name !== "mermaid") return false;
					sync(updated);
					return true;
				},
			};
		};
	},
});
