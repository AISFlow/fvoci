// Adapted from nuxt-ui-templates/editor app/composables/useEditorToolbar.ts
// at 60886bda1442549b90312ab5097a449eff634fd1 (MIT). FVOCI owns commands,
// permissions and the editor instance; these are presentation groups only.
import type { EditorToolbarItem } from "@nuxt/ui";
import { type TiptapEditor, insertSlashHere, useEditorState } from "@fvoci/editor/vue";
import { t } from "@fvoci/i18n";
import { computed } from "vue";

export const MARKS = ["bold", "italic", "underline", "strike", "code"] as const;
const ICONS = ["i-lucide-bold", "i-lucide-italic", "i-lucide-underline", "i-lucide-strikethrough", "i-lucide-code"] as const;

/** Commands must not interrupt an active composition or a revoked editor. */
export function canUseToolbar(editor: TiptapEditor): boolean {
  return !editor.isDestroyed && editor.isEditable && !editor.view.composing;
}

export function useEditorToolbar(editor: TiptapEditor) {
  const state = useEditorState(editor, (current) => ({
    editable: current.isEditable,
    undo: current.can().undo(),
    redo: current.can().redo(),
    marks: Object.fromEntries(MARKS.map((mark) => [mark, current.isActive(mark)])),
    heading: [1, 2, 3].find((level) => current.isActive("heading", { level })) ?? 0,
    lists: Object.fromEntries(["bulletList", "orderedList", "taskList"].map((kind) => [kind, current.isActive(kind)])),
    link: current.isActive("link"),
    highlight: current.isActive("highlight"),
    align: ["left", "center", "right"].find((textAlign) => current.isActive({ textAlign })) ?? "left",
  }));
  const history = computed<EditorToolbarItem[]>(() => [{
    icon: "i-lucide-undo", tooltip: { text: t("editor.undo") },
    disabled: !state.value.editable || !state.value.undo,
    onClick: () => { if (canUseToolbar(editor)) editor.chain().focus().undo().run(); },
  }, {
    icon: "i-lucide-redo", tooltip: { text: t("editor.redo") },
    disabled: !state.value.editable || !state.value.redo,
    onClick: () => { if (canUseToolbar(editor)) editor.chain().focus().redo().run(); },
  }]);
  const insert: EditorToolbarItem = { slot: "insert", icon: "i-lucide-plus", tooltip: { text: t("editor.mobile.insert") } };
  const format: EditorToolbarItem[][] = [[
    { slot: "type", icon: "i-lucide-type" },
    { slot: "lists", icon: "i-lucide-list" },
  ], MARKS.map((mark, index) => ({
    kind: "mark" as const, mark, icon: ICONS[index],
    tooltip: { text: t(`editor.mark.${mark}`) },
  })), [
    { slot: "link", icon: "i-lucide-link" },
    { slot: "highlight", icon: "i-lucide-highlighter" },
    { slot: "more", icon: "i-lucide-ellipsis" },
  ]];
  function insertSlash(): void { if (canUseToolbar(editor)) insertSlashHere(editor); }
  function insertTrigger(trigger: "@" | ":"): void {
    if (canUseToolbar(editor)) editor.chain().focus().insertContent(trigger).run();
  }
  return { state, history, insert, format, insertSlash, insertTrigger };
}
