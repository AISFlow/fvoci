<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { ref } from "vue";
import {
  downloadDocumentDocx,
  downloadDocumentMarkdown,
  downloadDocumentPdf,
  downloadDocumentPptx,
} from "@/lib/share";

// Export the saved document (features/documents/document-export-menu.tsx):
// the live body is persisted first when the page can.
const props = defineProps<{
  workspaceId: string;
  documentId: string;
  title: string;
  projectId: string | null;
  persistNow?: () => Promise<void>;
}>();
const pending = ref(false);

type ExportKind = "md" | "pdf" | "docx" | "pptx";
const DOWNLOAD = {
  md: downloadDocumentMarkdown,
  pdf: downloadDocumentPdf,
  docx: downloadDocumentDocx,
  pptx: downloadDocumentPptx,
} as const;
const KINDS: readonly ExportKind[] = ["md", "pdf", "docx", "pptx"];
const LABEL = {
  md: "export.md",
  pdf: "export.pdf",
  docx: "export.docx",
  pptx: "export.pptx",
} as const;
const FAILED = {
  md: "export.md.failed",
  pdf: "export.pdf.failed",
  docx: "export.docx.failed",
  pptx: "export.pptx.failed",
} as const;

function runExport(kind: ExportKind): void {
  pending.value = true;
  void (props.persistNow?.() ?? Promise.resolve())
    .then(() => DOWNLOAD[kind](props.workspaceId, props.documentId, props.title, props.projectId))
    .catch(() => {
      window.alert(t(FAILED[kind]));
    })
    .finally(() => {
      pending.value = false;
    });
}
</script>

<template>
  <div class="document-export-menu flex flex-wrap gap-2">
    <UButton
      v-for="kind in KINDS"
      :key="kind"
      size="sm"
      variant="outline"
      color="neutral"
      :disabled="pending"
      @click="runExport(kind)"
    >
      {{ t(LABEL[kind]) }}
    </UButton>
  </div>
</template>
