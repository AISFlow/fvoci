<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { api, ensureOk } from "@/lib/api";
import "@/features/settings/settings-shell.css";

const props = defineProps<{ workspaceId: string; canManage: boolean }>();

async function download(): Promise<void> {
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
  document.body.appendChild(link);
  link.click();
  link.remove();
  URL.revokeObjectURL(href);
}
</script>

<template>
  <section v-if="canManage" class="settings-section">
    <h2 class="settings-section__title">{{ t("export.workspace") }}</h2>
    <UButton type="button" @click="download">{{ t("export.workspace") }}</UButton>
  </section>
</template>
