<script setup lang="ts">
// Adapted from the official editor/LinkPopover.vue at 60886bda (MIT).
// FVOCI's popover owns portal/focus/Escape/Tab and retains the selection.
import { type TiptapEditor, useEditorState } from "@fvoci/editor/vue";
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import UInput from "@nuxt/ui/components/Input.vue";
import { shallowRef } from "vue";
import ToolbarPopover from "./ToolbarPopover.vue";
import { canUseToolbar } from "./useEditorToolbar";
const props = defineProps<{ editor: TiptapEditor; side: "top" | "bottom" }>();
const state = useEditorState(props.editor, (editor) => ({
  editable: editor.isEditable,
  active: editor.isActive("link"),
}));
const url = shallowRef("");
const composing = shallowRef<boolean>(false);
function readLink(): void {
  composing.value = false;
  const href: unknown = props.editor.getAttributes("link").href;
  url.value = typeof href === "string" ? href : "";
}
function setLink(close: () => void): void {
  if (composing.value || !canUseToolbar(props.editor) || !url.value.trim()) return;
  // Keep a non-empty selection's exact range; collapsed links edit the whole
  // existing link, as in the template. Never rewrite the shared document.
  let chain = props.editor.chain().focus();
  if (props.editor.state.selection.empty) chain = chain.extendMarkRange("link");
  chain.setLink({ href: url.value.trim() }).run();
  close();
}
function removeLink(close: () => void): void {
  if (!canUseToolbar(props.editor)) return;
  props.editor
    .chain()
    .focus()
    .extendMarkRange("link")
    .unsetLink()
    .setMeta("preventAutolink", true)
    .run();
  close();
}
function onKeydown(event: KeyboardEvent, close: () => void): void {
  if (event.key !== "Enter") return;
  // Enter confirms Korean IME before it can apply a URL. keyCode 229 also
  // covers browsers that finish composition before reporting the key.
  // Keep the existing IME fallback when composition ends before keydown.
  // eslint-disable-next-line @typescript-eslint/no-deprecated
  if (event.isComposing || composing.value || event.keyCode === 229) {
    event.preventDefault();
    return;
  }
  event.preventDefault();
  setLink(close);
}
</script>

<template>
  <ToolbarPopover
    :editor="editor"
    kind="dialog"
    :label="t('editor.link')"
    :side="side"
    @opening="readLink"
  >
    <template #trigger="{ open, id }">
      <UButton
        icon="i-lucide-link"
        color="neutral"
        variant="ghost"
        :aria-label="t('editor.link')"
        :aria-pressed="state.active"
        :disabled="!state.editable"
        aria-haspopup="dialog"
        :aria-expanded="open"
        :aria-controls="id"
      />
    </template>
    <template #default="{ close }">
      <form class="flex flex-wrap items-center gap-1" @submit.prevent="setLink(close)">
        <UInput
          v-model="url"
          name="url"
          type="text"
          aria-label="URL"
          placeholder="https://"
          size="sm"
          @keydown="onKeydown($event, close)"
          @compositionstart="composing = true"
          @compositionend="composing = false"
        />
        <UButton
          type="submit"
          icon="i-lucide-corner-down-left"
          size="sm"
          :aria-label="t('editor.link.apply')"
          >{{ t("editor.link.apply") }}</UButton
        >
        <UButton
          v-if="state.active"
          type="button"
          icon="i-lucide-unlink"
          color="neutral"
          variant="ghost"
          :aria-label="t('editor.format.clear')"
          @click="removeLink(close)"
        />
      </form>
    </template>
  </ToolbarPopover>
</template>
