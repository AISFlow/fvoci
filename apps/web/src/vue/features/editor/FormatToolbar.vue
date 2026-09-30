<script setup lang="ts">
import { type TiptapEditor, useEditorState } from "@fvoci/editor/vue";
import { type I18nKey, t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import UInput from "@nuxt/ui/components/Input.vue";
import { computed, shallowRef } from "vue";
import MenuItem from "./MenuItem.vue";
import ToolbarPopover from "./ToolbarPopover.vue";

// The formatting controls of the selection bubble and the mobile toolbar
// (react/format-toolbar.tsx and tiptap-ui/*): block type, lists, marks, link,
// highlight, and a menu with text alignment and clear formatting.
const props = withDefaults(defineProps<{ editor: TiptapEditor; side?: "top" | "bottom" }>(), { side: "bottom" });

const LEVELS = [1, 2, 3] as const;
const MARKS = ["bold", "italic", "underline", "strike", "code"] as const;
const MARK_LABEL = { bold: "B", italic: "I", underline: "U", strike: "S", code: "</>" } as const;
const LISTS = [
  { type: "bulletList", key: "editor.block.bullet" },
  { type: "orderedList", key: "editor.block.ordered" },
  { type: "taskList", key: "editor.block.task" },
] as const;
const ALIGNS = [
  { align: "left", key: "editor.align.left" },
  { align: "center", key: "editor.align.center" },
  { align: "right", key: "editor.align.right" },
] as const;
const HIGHLIGHTS: ReadonlyArray<{ key: I18nKey; value: string }> = [
  { key: "editor.color.yellow", value: "var(--accent)" },
  { key: "editor.color.red", value: "var(--destructive)" },
  { key: "editor.color.muted", value: "var(--muted)" },
];

const state = useEditorState(props.editor, (editor) => ({
  heading: LEVELS.find((level) => editor.isActive("heading", { level })) ?? 0,
  marks: Object.fromEntries(MARKS.map((mark) => [mark, editor.isActive(mark)])) as Record<(typeof MARKS)[number], boolean>,
  lists: Object.fromEntries(LISTS.map(({ type }) => [type, editor.isActive(type)])) as Record<
    (typeof LISTS)[number]["type"],
    boolean
  >,
  aligns: Object.fromEntries(ALIGNS.map(({ align }) => [align, editor.isActive({ textAlign: align })])) as Record<
    (typeof ALIGNS)[number]["align"],
    boolean
  >,
  link: editor.isActive("link"),
  highlight: editor.isActive("highlight"),
}));

const headingTrigger = computed(() =>
  state.value.heading ? `H${state.value.heading}▾` : `${t("editor.block.paragraph")}▾`,
);

const chain = () => props.editor.chain().focus();

/** WHY: a toolbar button's default mousedown would steal focus and drop the
 * ProseMirror caret (react/menu-keyboard.ts preventSelectionLoss). Inputs
 * in a dialog still take focus. */
function keepEditorFocus(event: MouseEvent): void {
  if (event.target instanceof HTMLElement && event.target.closest("button")) {
    event.preventDefault();
  }
}

function toggleList(type: (typeof LISTS)[number]["type"]): void {
  if (type === "bulletList") chain().toggleBulletList().run();
  else if (type === "orderedList") chain().toggleOrderedList().run();
  else chain().toggleTaskList().run();
}

const href = shallowRef("");

function readLink(): void {
  const current = props.editor.getAttributes("link").href;
  href.value = typeof current === "string" ? current : "";
}

function applyLink(): void {
  const next = href.value.trim();
  if (next.length === 0) return;
  chain().setLink({ href: next }).run();
}
</script>

<template>
  <div class="fvoci-ui-toolbar" role="toolbar" :aria-label="t('editor.format')" @mousedown="keepEditorFocus">
    <div class="fvoci-format-cluster">
      <ToolbarPopover :editor="editor" kind="menu" :label="t('editor.block.type')" :side="side">
        <template #trigger="{ open, id }">
          <UButton variant="ghost" color="neutral" aria-haspopup="menu" :aria-expanded="open" :aria-controls="id">
            {{ headingTrigger }}
          </UButton>
        </template>
        <template #default="{ close }">
          <MenuItem
            role="menuitemradio"
            :checked="state.heading === 0"
            @select="
              chain().setParagraph().run();
              close();
            "
          >
            {{ t("editor.block.paragraph") }}
          </MenuItem>
          <MenuItem
            v-for="level in LEVELS"
            :key="level"
            role="menuitemradio"
            :checked="state.heading === level"
            @select="
              chain().setHeading({ level }).run();
              close();
            "
          >
            H{{ level }}
          </MenuItem>
        </template>
      </ToolbarPopover>
      <ToolbarPopover :editor="editor" kind="menu" :label="t('editor.list')" :side="side">
        <template #trigger="{ open, id }">
          <UButton variant="ghost" color="neutral" aria-haspopup="menu" :aria-expanded="open" :aria-controls="id">
            {{ t("editor.list") }}
          </UButton>
        </template>
        <MenuItem
          v-for="list in LISTS"
          :key="list.type"
          role="menuitemcheckbox"
          :checked="state.lists[list.type]"
          @select="toggleList(list.type)"
        >
          {{ t(list.key) }}
        </MenuItem>
      </ToolbarPopover>
    </div>
    <div class="fvoci-format-cluster">
      <UButton
        v-for="mark in MARKS"
        :key="mark"
        variant="ghost"
        color="neutral"
        :aria-pressed="state.marks[mark]"
        :aria-label="t(`editor.mark.${mark}`)"
        @click="chain().toggleMark(mark).run()"
      >
        {{ MARK_LABEL[mark] }}
      </UButton>
    </div>
    <ToolbarPopover :editor="editor" kind="dialog" :label="t('editor.link')" :side="side" @opening="readLink">
      <template #trigger="{ open, id }">
        <UButton
          variant="ghost"
          color="neutral"
          :aria-pressed="state.link"
          aria-haspopup="dialog"
          :aria-expanded="open"
          :aria-controls="id"
        >
          {{ t("editor.link") }}
        </UButton>
      </template>
      <form class="flex items-center gap-1" @submit.prevent="applyLink">
        <UInput v-model="href" type="text" aria-label="URL" placeholder="https://" size="sm" />
        <UButton type="submit" size="sm">{{ t("editor.link.apply") }}</UButton>
      </form>
    </ToolbarPopover>
    <ToolbarPopover :editor="editor" kind="dialog" :label="t('editor.color.highlight')" :side="side">
      <template #trigger="{ open, id }">
        <UButton
          variant="ghost"
          color="neutral"
          :aria-pressed="state.highlight"
          aria-haspopup="dialog"
          :aria-expanded="open"
          :aria-controls="id"
        >
          {{ t("editor.color") }}
        </UButton>
      </template>
      <template #default="{ close }">
        <UButton
          v-for="color in HIGHLIGHTS"
          :key="color.key"
          variant="ghost"
          color="neutral"
          :aria-label="t(color.key)"
          @click="
            chain().toggleHighlight({ color: color.value }).run();
            close();
          "
        >
          {{ t(color.key) }}
        </UButton>
      </template>
    </ToolbarPopover>
    <ToolbarPopover :editor="editor" kind="menu" :label="t('editor.format')" :side="side">
      <template #trigger="{ open, id }">
        <UButton
          variant="ghost"
          color="neutral"
          :aria-label="t('editor.format')"
          aria-haspopup="menu"
          :aria-expanded="open"
          :aria-controls="id"
        >
          ⋮
        </UButton>
      </template>
      <template #default="{ close }">
        <MenuItem
          v-for="item in ALIGNS"
          :key="item.align"
          role="menuitemradio"
          :checked="state.aligns[item.align]"
          @select="chain().setTextAlign(item.align).run()"
        >
          {{ t(item.key) }}
        </MenuItem>
        <hr class="fvoci-vue-menu__separator" />
        <MenuItem
          @select="
            chain().unsetAllMarks().run();
            close();
          "
        >
          {{ t("editor.format.clear") }}
        </MenuItem>
      </template>
    </ToolbarPopover>
  </div>
</template>
