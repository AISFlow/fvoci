<script setup lang="ts">
import { t } from "@fvoci/i18n";
import { shallowRef } from "vue";
import { useEffectAfterRender } from "../../composables/useEffectAfterRender";
import ChunkText from "./ChunkText.vue";

/**
 * Search hit context for a laid-out document (source `SearchChunkSupplement`):
 * the stored extract text from `preview-html`, shown as plain text with the
 * hit highlighted above the layout viewer. It never replaces the layout, and
 * its failure never hides it.
 */
const props = defineProps<{ previewHtmlUrl: string; chunk: number }>();

type State =
  | { status: "loading" }
  | { status: "unavailable" }
  | { status: "error" }
  | { status: "text"; text: string };
const state = shallowRef<State>({ status: "loading" });

useEffectAfterRender([() => props.previewHtmlUrl], () => {
  const controller = new AbortController();
  let alive = true;
  const isAlive = (): boolean => alive;
  state.value = { status: "loading" };
  (async () => {
    const response = await fetch(props.previewHtmlUrl, {
      credentials: "include",
      signal: controller.signal,
    });
    if (!response.ok) {
      await response.body?.cancel();
      if (isAlive()) {
        state.value = {
          status: response.status === 404 || response.status === 413 ? "unavailable" : "error",
        };
      }
      return;
    }
    const payload: unknown = await response.json();
    const html =
      typeof payload === "object" && payload !== null && "html" in payload ? payload.html : null;
    if (!isAlive()) return;
    if (typeof html !== "string") {
      state.value = { status: "error" };
      return;
    }
    // The server escapes the text into one <pre> with no attributes; only
    // its text is used, never markup.
    const text = new DOMParser().parseFromString(html, "text/html").body.textContent;
    state.value = { status: "text", text };
  })().catch((error: unknown) => {
    if (!isAlive() || (error instanceof Error && error.name === "AbortError")) return;
    state.value = { status: "error" };
  });
  return () => {
    alive = false;
    controller.abort();
  };
});
</script>

<template>
  <section
    class="attachment-viewer__pane attachment-viewer__pane--supplement"
    data-chunk-supplement=""
  >
    <p class="attachment-viewer__status">{{ t("attachment.viewer.layoutNone") }}</p>
    <p v-if="state.status === 'loading'" class="attachment-viewer__status">{{
      t("attachment.preview.loading")
    }}</p>
    <ChunkText v-else-if="state.status === 'text'" :text="state.text" :chunk="chunk" />
    <p v-else class="attachment-viewer__status">
      {{
        state.status === "unavailable"
          ? t("attachment.viewer.previewUnavailable")
          : t("load.failed")
      }}
    </p>
  </section>
</template>
