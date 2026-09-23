import { mergeAttributes, Node } from "@tiptap/core";

export const Attachment = Node.create({
	name: "attachment",
	group: "block",
	atom: true,
	addAttributes() {
		return {
			id: { default: null },
			name: { default: "" },
			image: {
				default: false,
				parseHTML: (el) => el.getAttribute("data-image") === "true",
				renderHTML: (attributes) =>
					attributes.image === true ? { "data-image": "true" } : {},
			},
			width: { default: null },
			align: { default: null },
			caption: { default: null },
			/* WHY: C3 치수 예약 — 편집 세션이 첨부 메타에서 한 번 받아 적는다(널 = 모름). */
			previewWidth: { default: null },
			previewHeight: { default: null },
		};
	},
	parseHTML() {
		return [{ tag: "div[data-attachment]" }];
	},
	renderHTML({ HTMLAttributes }) {
		return [
			"div",
			mergeAttributes(HTMLAttributes, {
				"data-attachment": "",
				class: "afn-attachment",
			}),
		];
	},
});
