<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { computed, ref, shallowRef } from "vue";
import { PDF_ZOOM_MAX, PDF_ZOOM_MIN, zoomIn, zoomOut } from "@/features/attachments/pdf-limits";
import { downloadCapped, type ViewerPrefetch } from "@/features/attachments/viewer-download";
import { openXlsxInWorker, XlsxWorkerError, type RemoteXlsxBook } from "@/features/attachments/xlsx-client";
import { XLSX_MAX_BYTES } from "@/features/attachments/xlsx-limits";
import type { XlsxPage } from "@/features/attachments/xlsx-workbook";
import { useEffectAfterRender } from "../../composables/useEffectAfterRender";
import ViewerDownloadButton from "./ViewerDownloadButton.vue";
import ViewerErrorPane from "./ViewerErrorPane.vue";
import ViewerLoadingPane from "./ViewerLoadingPane.vue";
import ViewerZoomToolbar from "./ViewerZoomToolbar.vue";
import "@/features/attachments/xlsx-viewer.css";

type BookState =
  | { status: "loading" }
  | { status: "error"; message: string; retry: boolean }
  | { status: "ready"; book: RemoteXlsxBook };

/** The last page the worker returned, and the request it answers. */
type PageState = { sheetIndex: number; rowPage: number; colPage: number; page: XlsxPage | null };

function pageError(error: unknown): BookState {
  const timedOut = error instanceof XlsxWorkerError && error.reason === "timeout";
  return {
    status: "error",
    message: t(timedOut ? "attachment.viewer.previewUnavailable" : "load.failed"),
    retry: !timedOut,
  };
}

/**
 * Worksheet grid of an XLSX attachment (source `XlsxViewer`): sheet, row-page
 * (200) and column-page (64) navigation plus zoom over the cells' display
 * text. A chartsheet or other non-worksheet tab says it cannot be shown and
 * offers the original download; the other tabs stay reachable. The workbook
 * is parsed and paged in a dedicated worker (`xlsx-client.ts`) with a
 * wall-clock bound, and that worker is terminated on unmount or a new load.
 */
const props = defineProps<{ downloadUrl: string; prefetch?: ViewerPrefetch | undefined }>();

const generation = ref(0);
const state = shallowRef<BookState>({ status: "loading" });
const sheetIndex = ref(0);
const rowPage = ref(0);
const colPage = ref(0);
const zoom = ref(1);
const shown = shallowRef<PageState | null>(null);

useEffectAfterRender([() => props.downloadUrl, generation, () => props.prefetch], () => {
  const controller = new AbortController();
  let alive = true;
  state.value = { status: "loading" };
  sheetIndex.value = 0;
  rowPage.value = 0;
  colPage.value = 0;
  shown.value = null;
  let book: RemoteXlsxBook | null = null;
  void (async () => {
    try {
      const body = await (props.prefetch?.take(controller.signal) ??
        downloadCapped(props.downloadUrl, XLSX_MAX_BYTES, controller.signal));
      if (!alive) return;
      if (body.status === "failed") {
        state.value = { status: "error", message: t("load.failed"), retry: true };
        return;
      }
      if (body.status === "tooLarge") {
        state.value = { status: "error", message: t("attachment.viewer.previewUnavailable"), retry: false };
        return;
      }
      const opened = await openXlsxInWorker(body.bytes, { signal: controller.signal });
      if (opened.status === "ok") book = opened.book;
      if (!alive) {
        book?.close();
        return;
      }
      if (opened.status === "ok") {
        // The first page arrives with the book, so the grid never flashes empty.
        let first: PageState | null = null;
        if (opened.book.sheets[0]?.kind === "worksheet") {
          try {
            first = { sheetIndex: 0, rowPage: 0, colPage: 0, page: await opened.book.page(0, 0, 0) };
          } catch (error) {
            if (alive) state.value = pageError(error);
            return;
          }
          if (!alive) return;
        }
        shown.value = first;
        state.value = { status: "ready", book: opened.book };
      } else if (opened.status === "tooLarge") {
        state.value = { status: "error", message: t("attachment.viewer.previewUnavailable"), retry: false };
      } else {
        state.value = { status: "error", message: t("load.failed"), retry: true };
      }
    } catch (error) {
      if (!alive || (error instanceof Error && error.name === "AbortError")) return;
      state.value = { status: "error", message: t("load.failed"), retry: true };
    }
  })();
  return () => {
    alive = false;
    controller.abort();
    book?.close();
  };
});

const book = computed(() => (state.value.status === "ready" ? state.value.book : null));
const sheets = computed(() => book.value?.sheets ?? []);
const sheet = computed(() => book.value?.sheets[sheetIndex.value]);
const matchesShown = computed(
  () =>
    shown.value !== null &&
    shown.value.sheetIndex === sheetIndex.value &&
    shown.value.rowPage === rowPage.value &&
    shown.value.colPage === colPage.value,
);

useEffectAfterRender([book, sheet, sheetIndex, rowPage, colPage, matchesShown], () => {
  const current = book.value;
  if (!current || sheet.value?.kind !== "worksheet" || matchesShown.value) return;
  let alive = true;
  const index = sheetIndex.value;
  const row = rowPage.value;
  const col = colPage.value;
  current.page(index, row, col).then(
    (next) => {
      if (alive) shown.value = { sheetIndex: index, rowPage: row, colPage: col, page: next };
    },
    (error: unknown) => {
      if (alive) state.value = pageError(error);
    },
  );
  return () => {
    alive = false;
  };
});

// Until the requested page arrives, the previous page of the same sheet stays on screen.
const pending = computed(() => shown.value?.sheetIndex !== sheetIndex.value);
const page = computed(() => (pending.value ? null : (shown.value?.page ?? null)));

function retry(): void {
  generation.value += 1;
}

function showSheet(index: number): void {
  sheetIndex.value = index;
  rowPage.value = 0;
  colPage.value = 0;
}
</script>

<template>
  <ViewerLoadingPane v-if="state.status === 'loading'" />
  <ViewerErrorPane
    v-else-if="state.status === 'error'"
    :message="state.message"
    :download-url="downloadUrl"
    :retryable="state.retry"
    @retry="retry"
  />
  <ViewerErrorPane
    v-else-if="sheets.length === 0 || !sheet"
    :message="t('attachment.viewer.previewUnavailable')"
    :download-url="downloadUrl"
  />
  <div v-else class="attachment-viewer__pane" data-testid="xlsx-viewer">
    <ViewerZoomToolbar
      :zoom="zoom"
      :can-zoom-out="zoom > PDF_ZOOM_MIN"
      :can-zoom-in="zoom < PDF_ZOOM_MAX"
      @zoom-in="zoom = zoomIn(zoom)"
      @zoom-out="zoom = zoomOut(zoom)"
      @reset="zoom = 1"
    >
      <UButton size="sm" variant="outline" color="neutral" :disabled="sheetIndex <= 0" @click="showSheet(sheetIndex - 1)">
        {{ t("attachment.viewer.prevSheet") }}
      </UButton>
      <p class="attachment-viewer__page-label" aria-live="polite">
        {{ t("attachment.viewer.sheet") }}: {{ sheet.name }} ({{ sheetIndex + 1 }}/{{ sheets.length }})
      </p>
      <UButton
        size="sm"
        variant="outline"
        color="neutral"
        :disabled="sheetIndex + 1 >= sheets.length"
        @click="showSheet(sheetIndex + 1)"
      >
        {{ t("attachment.viewer.nextSheet") }}
      </UButton>
      <template v-if="page && page.rowPages > 1">
        <UButton
          size="sm"
          variant="outline"
          color="neutral"
          :disabled="page.rowPage <= 0"
          @click="rowPage = page.rowPage - 1"
        >
          {{ t("attachment.viewer.prevPage") }}
        </UButton>
        <p class="attachment-viewer__page-label" data-xlsx-row-page="">
          {{ t("attachment.viewer.page", { current: page.rowPage + 1, total: page.rowPages }) }}
        </p>
        <UButton
          size="sm"
          variant="outline"
          color="neutral"
          :disabled="page.rowPage + 1 >= page.rowPages"
          @click="rowPage = page.rowPage + 1"
        >
          {{ t("attachment.viewer.nextPage") }}
        </UButton>
      </template>
      <template v-if="page && page.colPages > 1">
        <UButton
          size="sm"
          variant="outline"
          color="neutral"
          :disabled="page.colPage <= 0"
          @click="colPage = page.colPage - 1"
        >
          {{ t("attachment.viewer.prevColumns") }}
        </UButton>
        <p class="attachment-viewer__page-label" data-xlsx-col-page="">
          {{ t("attachment.viewer.columns", { current: page.colPage + 1, total: page.colPages }) }}
        </p>
        <UButton
          size="sm"
          variant="outline"
          color="neutral"
          :disabled="page.colPage + 1 >= page.colPages"
          @click="colPage = page.colPage + 1"
        >
          {{ t("attachment.viewer.nextColumns") }}
        </UButton>
      </template>
    </ViewerZoomToolbar>
    <div
      v-if="sheet.kind === 'unsupported'"
      class="attachment-viewer__pane attachment-viewer__pane--center"
      data-xlsx-unsupported=""
    >
      <p class="attachment-viewer__status">{{ t("attachment.viewer.previewUnavailable") }}</p>
      <ViewerDownloadButton :href="downloadUrl" />
    </div>
    <div v-else class="attachment-viewer__page-wrap attachment-viewer__xlsx-body">
      <p v-if="pending" class="attachment-viewer__status">{{ t("attachment.preview.loading") }}</p>
      <p v-else-if="page === null || page.rows.length === 0" class="attachment-viewer__status">—</p>
      <table v-else class="attachment-viewer__xlsx-table" :style="{ zoom }">
        <tbody>
          <tr v-for="(cells, rowIndex) in page.rows" :key="page.box.minRow + rowIndex">
            <td v-for="(text, colIndex) in cells" :key="page.box.minCol + colIndex">{{ text }}</td>
          </tr>
        </tbody>
      </table>
    </div>
  </div>
</template>
