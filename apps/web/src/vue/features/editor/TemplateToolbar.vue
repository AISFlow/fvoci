<script setup lang="ts">
// The official template's grouped fixed/bubble toolbars, rendered against the
// existing editor. Always use the fixed rendering mode: FvociEditor already
// installs the one selection plugin at creation to preserve Yjs undo/redo.
import type { Editor as VueEditor } from "@tiptap/vue-3";
import { overlayOwner, type TiptapEditor } from "@fvoci/editor/vue";
import { type I18nKey, t } from "@fvoci/i18n";
import UEditorToolbar from "@nuxt/ui/components/EditorToolbar.vue";
import UTooltip from "@nuxt/ui/components/Tooltip.vue";
import UButton from "@nuxt/ui/components/Button.vue";
import { computed } from "vue";
import LinkPopover from "./LinkPopover.vue";
import MenuItem from "./MenuItem.vue";
import ToolbarPopover from "./ToolbarPopover.vue";
import { canUseToolbar, useEditorToolbar } from "./useEditorToolbar";

const props = withDefaults(
  defineProps<{ editor: TiptapEditor; mode?: "fixed" | "selection" | "mobile" }>(),
  { mode: "selection" },
);
// FvociEditor emits @tiptap/vue-3's Editor; the public alias intentionally
// exposes only the framework-neutral command surface. No second editor.
const toolbarEditor = props.editor as VueEditor;
const { state, history, insert, format, insertSlash, insertTrigger } = useEditorToolbar(
  props.editor,
);
const items = computed(() =>
  props.mode === "fixed"
    ? [history.value, [insert]]
    : props.mode === "mobile"
      ? [...format, [insert]]
      : format,
);
const side = computed(() => (props.mode === "mobile" ? "top" : "bottom"));
const headingLabel = computed(() =>
  state.value.heading ? `H${String(state.value.heading)}` : t("editor.block.paragraph"),
);
const lists = [
  { type: "bulletList", key: "editor.block.bullet", icon: "i-lucide-list" },
  { type: "orderedList", key: "editor.block.ordered", icon: "i-lucide-list-ordered" },
  { type: "taskList", key: "editor.block.task", icon: "i-lucide-list-check" },
] as const;
const highlights: ReadonlyArray<{ key: I18nKey; value: string }> = [
  { key: "editor.color.yellow", value: "var(--accent)" },
  { key: "editor.color.red", value: "var(--destructive)" },
  { key: "editor.color.muted", value: "var(--muted)" },
];
function run(command: () => void): void {
  if (canUseToolbar(props.editor)) command();
}
const chain = () => props.editor.chain().focus();
function toggleList(type: (typeof lists)[number]["type"]): void {
  run(() => {
    if (type === "bulletList") chain().toggleBulletList().run();
    else if (type === "orderedList") chain().toggleOrderedList().run();
    else chain().toggleTaskList().run();
  });
}
function keepSelection(event: MouseEvent): void {
  if (event.target instanceof Element && event.target.closest("button")) event.preventDefault();
}
</script>

<template>
  <div
    class="fvoci-template-toolbar"
    :class="`fvoci-template-toolbar--${mode}`"
    :data-mobile-toolbar="mode === 'mobile' ? '' : undefined"
    @mousedown="keepSelection"
  >
    <UEditorToolbar
      :editor="toolbarEditor"
      :items="items"
      layout="fixed"
      :aria-label="mode === 'fixed' ? t('editor.toolbar') : t('editor.format')"
      :ui="{ base: 'fvoci-template-toolbar__groups', group: 'shrink-0' }"
    >
      <template #item="{ item, isActive, isDisabled, onClick }">
        <UTooltip
          :text="item.tooltip?.text"
          :disabled="isDisabled(item)"
          :portal="overlayOwner(editor.view.dom)"
        >
          <UButton
            :icon="item.icon"
            size="sm"
            color="neutral"
            variant="ghost"
            active-variant="soft"
            :aria-label="item.tooltip?.text"
            :title="item.tooltip?.text"
            :aria-pressed="'mark' in item ? String(state.marks[item.mark]) : undefined"
            :active="isActive(item)"
            :disabled="isDisabled(item)"
            @click="run(() => onClick($event, item))"
          />
        </UTooltip>
      </template>
      <template #type>
        <ToolbarPopover :editor="editor" kind="menu" :label="t('editor.block.type')" :side="side">
          <template #trigger="{ open, id }">
            <UButton
              :label="headingLabel"
              trailing-icon="i-lucide-chevron-down"
              color="neutral"
              variant="ghost"
              :disabled="!state.editable"
              aria-haspopup="menu"
              :aria-expanded="open"
              :aria-controls="id"
            />
          </template>
          <template #default="{ close }">
            <MenuItem
              role="menuitemradio"
              :checked="state.heading === 0"
              @select="
                run(() => chain().setParagraph().run());
                close();
              "
              >{{ t("editor.block.paragraph") }}</MenuItem
            >
            <MenuItem
              v-for="level in [1, 2, 3] as const"
              :key="level"
              role="menuitemradio"
              :checked="state.heading === level"
              @select="
                run(() => chain().setHeading({ level }).run());
                close();
              "
              >H{{ level }}</MenuItem
            >
          </template>
        </ToolbarPopover>
      </template>
      <template #lists>
        <ToolbarPopover :editor="editor" kind="menu" :label="t('editor.list')" :side="side">
          <template #trigger="{ open, id }">
            <UButton
              icon="i-lucide-list"
              :aria-label="t('editor.list')"
              color="neutral"
              variant="ghost"
              :disabled="!state.editable"
              aria-haspopup="menu"
              :aria-expanded="open"
              :aria-controls="id"
            />
          </template>
          <MenuItem
            v-for="list in lists"
            :key="list.type"
            role="menuitemcheckbox"
            :checked="state.lists[list.type]"
            @select="toggleList(list.type)"
            >{{ t(list.key) }}</MenuItem
          >
        </ToolbarPopover>
      </template>
      <template #link><LinkPopover :editor="editor" :side="side" /></template>
      <template #highlight>
        <ToolbarPopover
          :editor="editor"
          kind="dialog"
          :label="t('editor.color.highlight')"
          :side="side"
        >
          <template #trigger="{ open, id }">
            <UButton
              icon="i-lucide-highlighter"
              :aria-label="t('editor.color')"
              :aria-pressed="state.highlight"
              color="neutral"
              variant="ghost"
              :disabled="!state.editable"
              aria-haspopup="dialog"
              :aria-expanded="open"
              :aria-controls="id"
            />
          </template>
          <template #default="{ close }">
            <UButton
              v-for="color in highlights"
              :key="color.key"
              :aria-label="t(color.key)"
              color="neutral"
              variant="ghost"
              @click="
                run(() => chain().toggleHighlight({ color: color.value }).run());
                close();
              "
              >{{ t(color.key) }}</UButton
            >
          </template>
        </ToolbarPopover>
      </template>
      <template #more>
        <ToolbarPopover :editor="editor" kind="menu" :label="t('editor.format')" :side="side">
          <template #trigger="{ open, id }">
            <UButton
              icon="i-lucide-ellipsis"
              :aria-label="t('editor.format')"
              color="neutral"
              variant="ghost"
              :disabled="!state.editable"
              aria-haspopup="menu"
              :aria-expanded="open"
              :aria-controls="id"
            />
          </template>
          <template #default="{ close }">
            <MenuItem
              v-for="align in ['left', 'center', 'right'] as const"
              :key="align"
              role="menuitemradio"
              :checked="state.align === align"
              @select="run(() => chain().setTextAlign(align).run())"
              >{{ t(`editor.align.${align}`) }}</MenuItem
            >
            <hr class="fvoci-vue-menu__separator" />
            <MenuItem
              @select="
                run(() => chain().unsetAllMarks().run());
                close();
              "
              >{{ t("editor.format.clear") }}</MenuItem
            >
          </template>
        </ToolbarPopover>
      </template>
      <template #insert>
        <UButton
          v-if="mode === 'mobile'"
          icon="i-lucide-plus"
          :aria-label="t('editor.mobile.insert')"
          color="neutral"
          variant="ghost"
          :disabled="!state.editable"
          @click="insertSlash"
        />
        <ToolbarPopover
          v-else
          :editor="editor"
          kind="menu"
          :label="t('editor.mobile.insert')"
          :side="side"
        >
          <template #trigger="{ open, id }">
            <UButton
              icon="i-lucide-plus"
              :aria-label="t('editor.mobile.insert')"
              color="neutral"
              variant="ghost"
              :disabled="!state.editable"
              aria-haspopup="menu"
              :aria-expanded="open"
              :aria-controls="id"
            />
          </template>
          <template #default="{ close }">
            <MenuItem
              @select="
                insertSlash();
                close();
              "
              >{{ t("editor.gutter.add") }}</MenuItem
            >
            <MenuItem
              @select="
                insertTrigger('@');
                close();
              "
              >@</MenuItem
            >
            <MenuItem
              @select="
                insertTrigger(':');
                close();
              "
              >:</MenuItem
            >
          </template>
        </ToolbarPopover>
      </template>
    </UEditorToolbar>
  </div>
</template>
