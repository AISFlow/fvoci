import type { Editor } from "@tiptap/core";
import { onBeforeUnmount, type Ref, shallowRef } from "vue";

/** `editor.isEditable` as a ref. VueNodeView.update skips re-rendering when
 * setEditable leaves the node object as it was, so node views follow the
 * editor's "update" event, which setEditable emits (the React views do the
 * same with useEditorState, react/node-views.tsx). */
export function useEditable(editor: Editor): Readonly<Ref<boolean>> {
	const editable = shallowRef(editor.isEditable);
	const sync = () => {
		editable.value = editor.isEditable;
	};
	editor.on("update", sync);
	onBeforeUnmount(() => {
		editor.off("update", sync);
	});
	return editable;
}
