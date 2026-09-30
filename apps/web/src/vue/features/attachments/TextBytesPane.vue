<script setup lang="ts">
import { shallowRef } from "vue";
import { ORIGINAL_FETCH_CREDENTIALS } from "@/features/attachments/viewer-download";
import { loadErrorMessage } from "@/lib/api";
import { useEffectAfterRender } from "../../composables/useEffectAfterRender";
import ChunkText from "./ChunkText.vue";
import ViewerErrorPane from "./ViewerErrorPane.vue";
import ViewerLoadingPane from "./ViewerLoadingPane.vue";

// A text attachment: its bytes as plain text, never markup.
const props = defineProps<{ downloadUrl: string; chunk?: number | undefined }>();

type State = { status: "loading" } | { status: "error"; message: string } | { status: "text"; text: string };
const state = shallowRef<State>({ status: "loading" });

useEffectAfterRender([() => props.downloadUrl], () => {
  let cancelled = false;
  state.value = { status: "loading" };
  void fetch(props.downloadUrl, { credentials: ORIGINAL_FETCH_CREDENTIALS })
    .then(async (response) => {
      if (!response.ok) throw new Error(String(response.status));
      return response.text();
    })
    .then((text) => {
      if (!cancelled) state.value = { status: "text", text };
    })
    .catch((error: unknown) => {
      if (!cancelled) state.value = { status: "error", message: loadErrorMessage(error) };
    });
  return () => {
    cancelled = true;
  };
});
</script>

<template>
  <ViewerLoadingPane v-if="state.status === 'loading'" />
  <ViewerErrorPane v-else-if="state.status === 'error'" :message="state.message" :download-url="downloadUrl" />
  <div v-else class="attachment-viewer__pane">
    <ChunkText :text="state.text" :chunk="chunk" />
  </div>
</template>
