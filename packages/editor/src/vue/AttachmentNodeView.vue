<script setup lang="ts">
import { NodeViewWrapper, nodeViewProps } from "@tiptap/vue-3";
import { computed } from "vue";
import {
  ATTACHMENT_ALIGNS,
  ATTACHMENT_WIDTH,
  type AttachmentUploadResult,
  type PreviewAttachment,
} from "../attachment-model.js";
import AttachmentBlock from "./AttachmentBlock.vue";
import { useEditable } from "./use-editable.js";

// The attachment node (react/node-views.tsx AttachmentNodeView).
const props = defineProps(nodeViewProps);
const editable = useEditable(props.editor);

const blockProps = computed<PreviewAttachment>(() => {
  const attrs = props.node.attrs;
  return {
    id: typeof attrs.id === "string" ? attrs.id : "",
    name: typeof attrs.name === "string" ? attrs.name : "",
    image: attrs.image === true,
    width: typeof attrs.width === "number" ? attrs.width : ATTACHMENT_WIDTH.max,
    align: ATTACHMENT_ALIGNS.find((a) => a === attrs.align) ?? "center",
    caption: typeof attrs.caption === "string" ? attrs.caption : "",
    previewWidth: typeof attrs.previewWidth === "number" ? attrs.previewWidth : 0,
    previewHeight: typeof attrs.previewHeight === "number" ? attrs.previewHeight : 0,
  };
});

/* WHY: #644 F8 — 업로드 완료 되쓰기는 사용자의 스텝이 아니다. y-tiptap 의 UndoManager 는
 * captureTransaction 에서 이 meta 를 보므로 Mod-z 가 삽입 자체를 되돌린다. */
function onUploaded(result: AttachmentUploadResult): void {
  const pos = props.getPos();
  if (pos === undefined) return;
  const { view } = props.editor;
  view.dispatch(
    view.state.tr
      .setNodeAttribute(pos, "id", result.id)
      .setNodeAttribute(pos, "name", result.name)
      .setNodeAttribute(pos, "image", result.image)
      .setMeta("addToHistory", false),
  );
}
</script>

<template>
  <NodeViewWrapper>
    <AttachmentBlock
      :block-props="blockProps"
      :read-only="!editable"
      @uploaded="onUploaded"
      @remove="props.deleteNode()"
    />
  </NodeViewWrapper>
</template>
