<script setup lang="ts">
import { t } from "@fvoci/i18n";
import { computed, inject, onBeforeUnmount, shallowRef, useTemplateRef, watch } from "vue";
import {
  type AttachmentMeta,
  type AttachmentUploadResult,
  attachmentBadge,
  decodeFilename,
  isStoredAttachmentId,
  type PreviewAttachment,
} from "../attachment-model.js";
import { attachmentBridgeKey } from "./keys.js";

// An attachment block (react/attachment-view.tsx AttachmentBlockView): the
// stored file card, or the picker / upload progress / error of a block that
// has no stored file yet. The web app registers no preview renderers, so a
// stored file is always the card.
const props = defineProps<{
  blockProps: PreviewAttachment;
  readOnly: boolean;
  /** A dropped or pasted file to upload at once. */
  initialFile?: File;
}>();
const emit = defineEmits<{
  uploaded: [result: AttachmentUploadResult];
  remove: [];
}>();

const bridge = inject(attachmentBridgeKey, null);
const picker = useTemplateRef<HTMLInputElement>("picker");
type Phase =
  | { kind: "idle" }
  | { kind: "uploading"; name: string; fraction: number }
  | { kind: "error"; name: string; file: File };
const phase = shallowRef<Phase>({ kind: "idle" });
let abort: AbortController | null = null;

const stored = computed(() => isStoredAttachmentId(props.blockProps.id));
const percent = computed(() => (phase.value.kind === "uploading" ? Math.round(phase.value.fraction * 100) : 0));

function isAbortError(err: unknown): boolean {
  return err instanceof Error && err.name === "AbortError";
}

function startUpload(file: File): void {
  if (!bridge) return;
  abort?.abort();
  const controller = new AbortController();
  abort = controller;
  phase.value = { kind: "uploading", name: file.name, fraction: 0 };
  void bridge
    .upload(
      file,
      (fraction) => {
        if (controller.signal.aborted) return;
        phase.value = { kind: "uploading", name: file.name, fraction };
      },
      controller.signal,
    )
    .then((result) => {
      if (controller.signal.aborted) return;
      phase.value = { kind: "idle" };
      emit("uploaded", result);
    })
    .catch((err: unknown) => {
      if (controller.signal.aborted || isAbortError(err)) return;
      phase.value = { kind: "error", name: file.name, file };
    });
}

if (props.initialFile) startUpload(props.initialFile);

/* WHY: #644 F7 — 노드 삭제·Mod-z·에디터 파괴로 뷰가 사라져도 in-flight 업로드는 끝까지 달려
 * 사라진 노드에 되쓰기를 시도한다. 언마운트에서 끊는다. */
onBeforeUnmount(() => abort?.abort());

function onFileChange(event: Event): void {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  input.value = "";
  if (file) startUpload(file);
  else emit("remove");
}

function retry(): void {
  if (phase.value.kind === "error") startUpload(phase.value.file);
}

function cancel(): void {
  abort?.abort();
  phase.value = { kind: "idle" };
  emit("remove");
}

// Size and type badge of a stored file (GET attachments/:id metadata).
const meta = shallowRef<AttachmentMeta | null>(null);
watch(
  () => (stored.value ? props.blockProps.id : null),
  (id, _previous, onCleanup) => {
    meta.value = null;
    if (!id || !bridge?.attachmentMeta) return;
    let cancelled = false;
    onCleanup(() => {
      cancelled = true;
    });
    bridge.attachmentMeta(id).then(
      (next) => {
        if (!cancelled) meta.value = next;
      },
      () => undefined,
    );
  },
  { immediate: true },
);
const badge = computed(() => attachmentBadge(meta.value?.sizeBytes, meta.value?.mime));
const storedName = computed(() => decodeFilename(props.blockProps.name) || t("editor.block.attachment"));
</script>

<template>
  <template v-if="stored">
    <a
      v-if="bridge"
      class="afn-attachment"
      data-state="stored"
      :href="bridge.downloadUrl(blockProps.id)"
      download
    >
      <span class="afn-attachment-icon" aria-hidden="true">{{ blockProps.image ? "🖼️" : "📎" }}</span>
      <span class="afn-attachment-name">{{ storedName }}</span>
      <span v-if="badge" class="afn-attachment-badge">{{ badge }}</span>
    </a>
    <div v-else class="afn-attachment" data-state="stored">
      <span class="afn-attachment-icon" aria-hidden="true">{{ blockProps.image ? "🖼️" : "📎" }}</span>
      <span class="afn-attachment-name">{{ storedName }}</span>
    </div>
  </template>
  <!-- WHY: #644 리뷰 #4 — 진행 중 업로드는 읽기 전용으로 뒤집혀도 진행률·취소를 보인다. -->
  <div v-else-if="phase.kind === 'uploading'" class="afn-attachment" data-state="uploading">
    <span class="afn-attachment-icon" aria-hidden="true">📎</span>
    <span class="afn-attachment-name">{{ phase.name }}</span>
    <span
      class="afn-attachment-progress"
      role="progressbar"
      :aria-label="phase.name"
      :aria-valuenow="percent"
      aria-valuemin="0"
      aria-valuemax="100"
    >
      <span class="afn-attachment-progress-bar" :style="{ width: `${percent}%` }" />
    </span>
    <span class="afn-attachment-percent">{{ percent }}%</span>
    <button type="button" class="afn-attachment-button min-h-11" @mousedown.prevent @click="cancel">
      {{ t("editor.attach.cancel") }}
    </button>
  </div>
  <div v-else-if="!bridge || readOnly" class="afn-attachment" data-state="placeholder">
    <span class="afn-attachment-icon" aria-hidden="true">📎</span>
    <span class="afn-attachment-name">{{ t("editor.attach.unselected") }}</span>
  </div>
  <div v-else-if="phase.kind === 'error'" class="afn-attachment" data-state="error" role="alert">
    <span class="afn-attachment-icon" aria-hidden="true">⚠️</span>
    <span class="afn-attachment-name">{{ phase.name }}</span>
    <span class="afn-attachment-error">{{ t("editor.attach.failed") }}</span>
    <button
      type="button"
      class="afn-attachment-button min-h-11"
      @mousedown.prevent
      @click="retry"
    >
      {{ t("editor.attach.retry") }}
    </button>
    <button type="button" class="afn-attachment-button min-h-11" @mousedown.prevent @click="emit('remove')">
      {{ t("editor.attach.remove") }}
    </button>
  </div>
  <template v-else>
    <input ref="picker" type="file" class="afn-attachment-input" @change="onFileChange" />
    <div class="afn-attachment" data-state="placeholder">
      <span class="afn-attachment-icon" aria-hidden="true">📎</span>
      <span class="afn-attachment-name">{{ t("editor.attach.unselected") }}</span>
      <button type="button" class="afn-attachment-button min-h-11" @mousedown.prevent @click="picker?.click()">
        {{ t("editor.attach.pick") }}
      </button>
      <button type="button" class="afn-attachment-button min-h-11" @mousedown.prevent @click="emit('remove')">
        {{ t("editor.attach.remove") }}
      </button>
    </div>
  </template>
</template>
