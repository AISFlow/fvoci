<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { ref } from "vue";
import { api, ensureOk, loadErrorMessage } from "@/lib/api";
import "@/features/settings/settings-shell.css";

const props = defineProps<{ workspaceId: string; canManage: boolean }>();
const pending = ref(false);
const error = ref<string | null>(null);

async function download(): Promise<void> {
  if (pending.value || !props.canManage) return;
  pending.value = true;
  error.value = null;
  try {
    const blob = await ensureOk(
      await api.GET("/api/v1/workspaces/{workspace_id}/export", {
        params: { path: { workspace_id: props.workspaceId } },
        parseAs: "blob",
      }),
    );
    const href = URL.createObjectURL(blob);
    const link = document.createElement("a");
    link.href = href;
    link.download = "fvoci-workspace.zip";
    link.rel = "noopener";
    try {
      document.body.appendChild(link);
      link.click();
    } finally {
      link.remove();
      URL.revokeObjectURL(href);
    }
  } catch (err) {
    error.value = loadErrorMessage(err);
  } finally {
    pending.value = false;
  }
}
</script>

<template>
  <section v-if="canManage" class="settings-section">
    <h2 class="settings-section__title">{{ t("export.workspace") }}</h2>
    <UButton type="button" :loading="pending" :disabled="pending" @click="download">{{ t("export.workspace") }}</UButton>
    <p v-if="error" role="alert" class="settings-notice settings-notice--danger">{{ error }}</p>
  </section>
</template>
