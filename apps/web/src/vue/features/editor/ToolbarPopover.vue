<script setup lang="ts">
import {
	focusFirstMenuItem,
	leaveMenu,
	moveMenuFocus,
	overlayOwner,
	restoreNativeSelection,
	type TiptapEditor,
} from "@fvoci/editor/vue";
import UPopover from "@nuxt/ui/components/Popover.vue";
import { computed, nextTick, shallowRef, useId, useTemplateRef, watch } from "vue";
import { menuContent } from "./menu-content";

// A popover of the format toolbar (react/tiptap-ui-primitive/popover.tsx
// and dropdown-menu.tsx): the trigger slot's button toggles it. A menu
// enters at its first item and moves with ↑/↓/Home/End; a dialog (the link
// field, the highlight colours) focuses its first field. Escape closes it and
// returns focus to the trigger, as does closing with nothing focused; after a
// command the editor keeps the focus the command gave it. Tab leaves a menu
// from its trigger. In the mobile toolbar it opens upwards.
const props = defineProps<{
	editor: TiptapEditor;
	kind: "menu" | "dialog";
	label: string;
	side: "top" | "bottom";
}>();
/** `opening` fires before the content renders, so it can read the editor. */
const emit = defineEmits<{ opening: [] }>();

const open = shallowRef(false);
const id = useId();
const host = useTemplateRef<HTMLElement>("host");
const content = useTemplateRef<HTMLElement>("content");
let escaped = false;
let tabbed = false;
let menuFocus: HTMLElement | null = null;

function rememberMenuItem(event: Event): void {
  if (props.kind !== "menu" || !(event.target instanceof Element)) return;
  const item = event.target.closest<HTMLElement>('[role^="menuitem"]');
  if (item && content.value?.contains(item)) menuFocus = item;
}

watch(
  open,
  (value) => {
    if (value) emit("opening");
  },
  { flush: "sync" },
);

function trigger(): HTMLElement | null {
  return host.value?.querySelector("button") ?? null;
}

function close(): void {
  open.value = false;
}

const options = computed(() =>
  menuContent({
    side: props.side,
    sideOffset: 4,
    onOpenAutoFocus: (event) => {
      if (props.kind !== "menu") return;
      event.preventDefault();
      focusFirstMenuItem(content.value);
    },
    onEscapeKeyDown: () => {
      escaped = true;
    },
    onFocusOutside: (event) => {
      // Checkbox/radio commands focus the editor to restore its selection.
      // Keep their menu open; a pointer outside still dismisses it normally.
      if (props.kind === "menu" && event.target instanceof Node && props.editor.view.dom.contains(event.target)) {
        event.preventDefault();
        // Keep Escape/Tab and arrow navigation in the menu after the command.
        void nextTick(() => {
          if (open.value && menuFocus && content.value?.contains(menuFocus)) menuFocus.focus({ preventScroll: true });
        });
      }
    },
    onCloseAutoFocus: (event) => {
      event.preventDefault();
      if (tabbed) {
        tabbed = false;
        return;
      }
      const doc = host.value?.ownerDocument ?? document;
      if (escaped || doc.activeElement === doc.body) {
        escaped = false;
        trigger()?.focus({ preventScroll: true });
        if (props.kind === "dialog") restoreNativeSelection(props.editor);
      }
    },
  }),
);

function onTab(event: KeyboardEvent): void {
  if (props.kind !== "menu" || event.key !== "Tab") return;
  tabbed = true;
  leaveMenu(event, trigger(), close);
}

function onKeydown(event: KeyboardEvent): void {
  if (props.kind === "menu") moveMenuFocus(content.value, event);
}
</script>

<template>
  <span ref="host" class="fvoci-ui-popover" :data-open="String(open)">
    <UPopover v-model:open="open" :portal="host ? overlayOwner(host) : true" :content="options">
      <slot name="trigger" :open="open" :id="id" />
      <template #content>
        <div
          :id="id"
          ref="content"
          :role="kind"
          :aria-label="label"
          :aria-labelledby="undefined"
          :class="kind === 'menu' ? 'fvoci-vue-menu' : 'fvoci-vue-popover'"
          @keydown.capture="onTab"
          @keydown="onKeydown"
          @focusin="rememberMenuItem"
          @click="rememberMenuItem"
        >
          <slot :close="close" />
        </div>
      </template>
    </UPopover>
  </span>
</template>
