<script setup lang="ts">
import {
  convertBlock,
  copyText,
  deleteBlock,
  duplicateBlock,
  moveBlock,
  type TiptapEditor,
} from "@fvoci/editor/vue";
import { type I18nKey, t } from "@fvoci/i18n";
import { shallowRef } from "vue";
import { canUseToolbar } from "./useEditorToolbar";
import MenuItem from "./MenuItem.vue";
import PointMenu from "./PointMenu.vue";

// The block menu of the gutter (react/block-menu.tsx): convert the block,
// duplicate, move up/down, colour, copy its link, delete. Each command is an
// editor command, so Yjs carries it to peers.
const props = defineProps<{ editor: TiptapEditor; pos: number; x: number; y: number; id?: string }>();
const emit = defineEmits<{ close: [] }>();

type ConvertKind = Parameters<typeof convertBlock>[2];
const CONVERT: ReadonlyArray<{ key: I18nKey; kind: ConvertKind }> = [
  { key: "editor.block.paragraph", kind: "paragraph" },
  { key: "editor.block.heading1", kind: "heading1" },
  { key: "editor.block.heading2", kind: "heading2" },
  { key: "editor.block.heading3", kind: "heading3" },
  { key: "editor.block.blockquote", kind: "blockquote" },
  { key: "editor.block.bullet", kind: "bulletList" },
  { key: "editor.block.ordered", kind: "orderedList" },
  { key: "editor.block.task", kind: "taskList" },
  { key: "editor.mark.code", kind: "codeBlock" },
  { key: "editor.block.callout", kind: "callout" },
  { key: "editor.block.toggle", kind: "details" },
];

const copyFailed = shallowRef(false);

function run(command: () => void): void {
  if (!canUseToolbar(props.editor)) return;
  command();
  emit("close");
}

function colour(): void {
  const node = props.editor.state.doc.nodeAt(props.pos);
  if (!node) return;
  props.editor
    .chain()
    .focus()
    .setTextSelection({ from: props.pos + 1, to: props.pos + node.nodeSize - 1 })
    .setColor("var(--destructive)")
    .run();
}

/** Copies `#<block id>`; a failure stays visible and the menu stays open for a retry. */
function copyLink(): void {
  const id = props.editor.state.doc.nodeAt(props.pos)?.attrs.id;
  if (typeof id !== "string" || id.length === 0) return;
  copyFailed.value = false;
  copyText(`#${id}`).then(
    () => emit("close"),
    () => {
      copyFailed.value = true;
    },
  );
}
</script>

<template>
  <PointMenu :id="id" :x="x" :y="y" :owner="editor.view.dom" :label="t('editor.menu.block')" @close="emit('close')">
    <div class="fvoci-vue-menu__group">
      <MenuItem v-for="item in CONVERT" :key="item.kind" @select="run(() => convertBlock(editor, pos, item.kind))">
        {{ t(item.key) }}
      </MenuItem>
    </div>
    <hr class="fvoci-vue-menu__separator" />
    <div class="fvoci-vue-menu__group">
      <MenuItem @select="run(() => duplicateBlock(editor, pos))">{{ t("editor.menu.duplicate") }}</MenuItem>
      <MenuItem @select="run(() => moveBlock(editor, pos, -1))">{{ t("editor.menu.up") }}</MenuItem>
      <MenuItem @select="run(() => moveBlock(editor, pos, 1))">{{ t("editor.menu.down") }}</MenuItem>
      <MenuItem @select="run(colour)">{{ t("editor.color") }}</MenuItem>
      <MenuItem @select="copyLink">{{ t("editor.menu.copyLink") }}</MenuItem>
    </div>
    <p v-if="copyFailed" role="alert" class="fvoci-vue-menu__alert">{{ t("editor.copy.failed") }}</p>
    <hr class="fvoci-vue-menu__separator" />
    <MenuItem @select="run(() => deleteBlock(editor, pos))">{{ t("editor.menu.delete") }}</MenuItem>
  </PointMenu>
</template>
