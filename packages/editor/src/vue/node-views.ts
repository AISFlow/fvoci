import { VueNodeViewRenderer } from "@tiptap/vue-3";
import type { FvociNodeViews } from "../editor-extensions.js";
import AttachmentNodeView from "./AttachmentNodeView.vue";
import EmbedNodeView from "./EmbedNodeView.vue";
import MathInlineNodeView from "./MathInlineNodeView.vue";
import MathNodeView from "./MathNodeView.vue";

/** The Vue host's node views, applied by createFvociEditorExtensions only as
 * `addNodeView` (test/vue-node-views.test.ts checks the schema stays the
 * yjs seed contract). Mermaid keeps its plain source view (nodes/mermaid.ts)
 * until a Vue diagram view exists. */
export const VUE_NODE_VIEWS: FvociNodeViews = {
	math: () => VueNodeViewRenderer(MathNodeView),
	mathInline: () => VueNodeViewRenderer(MathInlineNodeView),
	embed: () => VueNodeViewRenderer(EmbedNodeView),
	attachment: () => VueNodeViewRenderer(AttachmentNodeView),
};
