<script setup lang="ts">
import { type GutterHandle, type TiptapEditor, useBlockGutter } from "@fvoci/editor/vue";
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useId } from "vue";
import BlockMenu from "./BlockMenu.vue";

// The block gutter (react/gutter.tsx): "+" and the drag handle "⠿" beside
// the hovered block (rendered into the editor's drag handle element, which
// the drag handle plugin places and shows), a keyboard button for the
// caret's block that shows when focused, and the block menu. On narrow
// screens the stylesheet hides the gutter unless a touch is held.
const props = defineProps<{ editor: TiptapEditor; gutter: GutterHandle }>();
const { keyboardPos, menu, openFromKeyboard, closeMenu, plus, drag } = useBlockGutter(props.editor, props.gutter);
const menuId = useId();

function onKeyboardClick(event: MouseEvent): void {
  if (event.currentTarget instanceof HTMLElement) openFromKeyboard(event.currentTarget);
}
</script>

<template>
  <UButton
    class="fvoci-gutter-keyboard"
    variant="ghost"
    color="neutral"
    :disabled="keyboardPos < 0"
    :aria-label="t('editor.gutter.move')"
    aria-haspopup="menu"
    :aria-expanded="menu !== null"
    :aria-controls="menu ? menuId : undefined"
    @click="onKeyboardClick"
  >
    ⠿
  </UButton>
  <Teleport :to="gutter.element">
    <UButton
      data-gutter="plus"
      icon="i-lucide-plus"
      size="sm"
      variant="ghost"
      color="neutral"
      :aria-label="t('editor.gutter.add')"
      @pointerdown="plus.pointerdown"
      @pointerup="plus.pointerup"
      @click="plus.click"
    >
    </UButton>
    <UButton
      data-gutter="drag"
      icon="i-lucide-grip-vertical"
      :active="menu !== null"
      active-variant="soft"
      size="sm"
      variant="ghost"
      color="neutral"
      :aria-label="t('editor.gutter.move')"
      aria-haspopup="menu"
      :aria-expanded="menu !== null"
      :aria-controls="menu ? menuId : undefined"
      @pointerdown="drag.pointerdown"
      @pointermove="drag.pointermove"
      @dragstart="drag.dragstart"
      @pointercancel="drag.pointercancel"
      @click="drag.click"
    >
    </UButton>
  </Teleport>
  <BlockMenu
    v-if="menu && menu.pos >= 0"
    :id="menuId"
    :editor="editor"
    :pos="menu.pos"
    :x="menu.x"
    :y="menu.y"
    @close="closeMenu"
  />
</template>
