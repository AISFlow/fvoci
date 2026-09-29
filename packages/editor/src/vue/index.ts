// The Vue editor host and its node views (@fvoci/editor/vue).
export { default as FvociEditor } from "./FvociEditor.vue";
export { default as SafeHtml } from "./SafeHtml.vue";
export {
	attachmentBridgeKey,
	entityResolverKey,
	type UrlEmbedComponent,
	urlEmbedKey,
} from "./keys.js";
export { VUE_NODE_VIEWS } from "./node-views.js";
export type { Editor as TiptapEditor } from "@tiptap/core";
export { collabCaretRender, type FvociCollabUser } from "../editor-extensions.js";
