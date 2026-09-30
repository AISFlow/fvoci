<script setup lang="ts">
import { focusFirstMenuItem, leaveMenu, moveMenuFocus, overlayOwner } from "@fvoci/editor/vue";
import UPopover from "@nuxt/ui/components/Popover.vue";
import { nextTick, onBeforeUnmount, useTemplateRef, watch } from "vue";
import { menuContent } from "./menu-content";

// A menu opened at a point (react/point-menu.tsx): the block menu and the
// table menu. It enters at its first item, ↑/↓/Home/End move through the
// items, Escape closes it and returns focus to where it was, Tab closes it
// and moves on from there, and scrolling anything but the menu closes it.
// After a command the editor keeps the focus the command gave it.
const props = defineProps<{ x: number; y: number; owner: HTMLElement; label: string; id?: string }>();
const emit = defineEmits<{ close: [] }>();

const menu = useTemplateRef<HTMLElement>("menu");
const doc = props.owner.ownerDocument;
const restore = doc.activeElement;
let escaped = false;
let tabbed = false;
const reference = { getBoundingClientRect: () => new DOMRect(props.x, props.y, 0, 0) };

// Reka positions the popover (and focusing the first item) can scroll the
// document in the same turn the menu appears. Closing on that opening scroll
// would drop the menu before the first item is reachable (the second keyboard
// open in workspace-wiki-vue-controls). Arm after layout.
let armed = false;
function armScrollClose(): void {
  armed = false;
  requestAnimationFrame(() => {
    requestAnimationFrame(() => {
      armed = true;
    });
  });
}
armScrollClose();
const onScroll = (event: Event) => {
  if (!armed) return;
  if (event.target instanceof Node && menu.value?.contains(event.target)) return;
  emit("close");
};
doc.addEventListener("scroll", onScroll, true);
onBeforeUnmount(() => doc.removeEventListener("scroll", onScroll, true));

watch(menu, (el) => {
  if (!el) return;
  armScrollClose();
  void nextTick(() => focusFirstMenuItem(el));
});

const content = menuContent({
  side: "bottom",
  sideOffset: 0,
  onOpenAutoFocus: (event) => {
    event.preventDefault();
    void nextTick(() => focusFirstMenuItem(menu.value));
  },
  onEscapeKeyDown: () => {
    escaped = true;
  },
  onCloseAutoFocus: (event) => {
    event.preventDefault();
    if (tabbed) return;
    if (restore instanceof HTMLElement && (escaped || doc.activeElement === doc.body)) {
      restore.focus({ preventScroll: true });
    }
  },
});

function onTab(event: KeyboardEvent): void {
  if (event.key !== "Tab") return;
  tabbed = true;
  leaveMenu(event, restore, () => emit("close"));
}
</script>

<template>
  <UPopover
    :open="true"
    :portal="overlayOwner(owner)"
    :reference="reference"
    :content="content"
    @update:open="(open: boolean) => open || emit('close')"
  >
    <template #content>
      <div
        :id="id"
        ref="menu"
        role="menu"
        :aria-label="label"
        :aria-labelledby="undefined"
        class="fvoci-vue-menu"
        @keydown.capture="onTab"
        @keydown="moveMenuFocus(menu, $event)"
      >
        <slot />
      </div>
    </template>
  </UPopover>
</template>
