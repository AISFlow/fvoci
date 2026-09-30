<script setup lang="ts">
import {
  addColumn,
  addRow,
  deleteCurrentTable,
  equalizeColumns,
  mergeSelectedCells,
  setCellAlign,
  setCellBackground,
  splitSelectedCells,
  type TiptapEditor,
  toggleTableHeaderColumn,
  toggleTableHeaderRow,
  useTableHandles,
} from "@fvoci/editor/vue";
import { type I18nKey, t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useId } from "vue";
import { canUseToolbar } from "./useEditorToolbar";
import MenuItem from "./MenuItem.vue";
import PointMenu from "./PointMenu.vue";

// The table handles (react/table-handles.tsx), over the table the caret is
// in: the table handle (drag it to move the table, click it for the menu),
// the column and row handles (the menu), and "+" for a column and a row.
// Every action is an editor command, so Yjs carries it to peers.
const props = defineProps<{ editor: TiptapEditor }>();
const { box, menu, closeMenu, openMenu, drag, moveTable } = useTableHandles(props.editor);
const menuId = useId();

const BACKGROUNDS: ReadonlyArray<{ key: I18nKey; value: string | null }> = [
  { key: "editor.color.none", value: null },
  { key: "editor.color.muted", value: "var(--muted)" },
  { key: "editor.color.accent", value: "var(--accent)" },
  { key: "editor.color.danger", value: "var(--destructive)" },
];

function run(command: () => void): void {
  if (!canUseToolbar(props.editor)) return;
  command();
  closeMenu();
}
</script>

<template>
  <div
    v-if="box"
    class="fvoci-table-handles"
    data-table-handles=""
    :style="{
      left: `${box.left}px`,
      top: `${box.top}px`,
      width: `${box.width}px`,
      height: `${box.height}px`,
    }"
  >
    <UButton
      data-table-handle="table"
      class="fvoci-table-handle--table"
      variant="outline"
      color="neutral"
      :aria-label="t('editor.table.handle')"
      aria-haspopup="menu"
      :aria-expanded="menu !== null"
      :aria-controls="menu ? menuId : undefined"
      @pointerdown="drag.pointerdown"
      @pointermove="drag.pointermove"
      @pointerup="drag.pointerup"
      @pointercancel="drag.pointercancel"
      @click="openMenu"
    >
      {{ t("editor.block.table") }}
    </UButton>
    <UButton
      data-table-handle="col"
      class="fvoci-table-handle--col"
      variant="outline"
      color="neutral"
      :aria-label="t('editor.table.colHandle')"
      aria-haspopup="menu"
      :aria-expanded="menu !== null"
      :aria-controls="menu ? menuId : undefined"
      @click="openMenu"
    >
      ↕
    </UButton>
    <UButton
      data-table-handle="row"
      class="fvoci-table-handle--row"
      variant="outline"
      color="neutral"
      :aria-label="t('editor.table.rowHandle')"
      aria-haspopup="menu"
      :aria-expanded="menu !== null"
      :aria-controls="menu ? menuId : undefined"
      @click="openMenu"
    >
      ↔
    </UButton>
    <UButton
      data-table-handle="col-plus"
      class="fvoci-table-handle--col-plus"
      variant="outline"
      color="neutral"
      :aria-label="t('editor.table.addCol')"
      @click="addColumn(editor)"
    >
      +
    </UButton>
    <UButton
      data-table-handle="row-plus"
      class="fvoci-table-handle--row-plus"
      variant="outline"
      color="neutral"
      :aria-label="t('editor.table.addRow')"
      @click="addRow(editor)"
    >
      +
    </UButton>
  </div>
  <PointMenu
    v-if="box && menu"
    :id="menuId"
    :x="menu.x"
    :y="menu.y"
    :owner="editor.view.dom"
    :label="t('editor.block.table')"
    @close="closeMenu"
  >
    <div class="fvoci-vue-menu__group">
      <MenuItem @select="run(() => addColumn(editor))">{{ t("editor.table.insertCol") }}</MenuItem>
      <MenuItem @select="run(() => editor.chain().focus().deleteColumn().run())">{{
        t("editor.table.deleteCol")
      }}</MenuItem>
      <MenuItem @select="run(() => addRow(editor))">{{ t("editor.table.insertRow") }}</MenuItem>
      <MenuItem @select="run(() => editor.chain().focus().deleteRow().run())">{{
        t("editor.table.deleteRow")
      }}</MenuItem>
    </div>
    <hr class="fvoci-vue-menu__separator" />
    <div class="fvoci-vue-menu__group">
      <MenuItem @select="run(() => moveTable(-1))">{{ t("editor.table.moveUp") }}</MenuItem>
      <MenuItem @select="run(() => moveTable(1))">{{ t("editor.table.moveDown") }}</MenuItem>
      <MenuItem @select="run(() => toggleTableHeaderRow(editor))">{{
        t("editor.table.headerRow")
      }}</MenuItem>
      <MenuItem @select="run(() => toggleTableHeaderColumn(editor))">{{
        t("editor.table.headerCol")
      }}</MenuItem>
    </div>
    <hr class="fvoci-vue-menu__separator" />
    <div class="fvoci-vue-menu__group">
      <MenuItem @select="run(() => setCellAlign(editor, 'left'))">{{
        t("editor.align.left")
      }}</MenuItem>
      <MenuItem @select="run(() => setCellAlign(editor, 'center'))">{{
        t("editor.align.center")
      }}</MenuItem>
      <MenuItem @select="run(() => setCellAlign(editor, 'right'))">{{
        t("editor.align.right")
      }}</MenuItem>
      <MenuItem
        v-for="item in BACKGROUNDS"
        :key="item.key"
        @select="run(() => setCellBackground(editor, item.value))"
      >
        {{ t("editor.table.background", { name: t(item.key) }) }}
      </MenuItem>
      <MenuItem @select="run(() => mergeSelectedCells(editor))">{{
        t("editor.table.merge")
      }}</MenuItem>
      <MenuItem @select="run(() => splitSelectedCells(editor))">{{
        t("editor.table.split")
      }}</MenuItem>
      <MenuItem @select="run(() => equalizeColumns(editor))">{{
        t("editor.table.equalize")
      }}</MenuItem>
    </div>
    <hr class="fvoci-vue-menu__separator" />
    <MenuItem @select="run(() => deleteCurrentTable(editor))">{{
      t("editor.table.delete")
    }}</MenuItem>
  </PointMenu>
</template>
