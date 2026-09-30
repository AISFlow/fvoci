<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, watchEffect } from "vue";
import { useRouter } from "vue-router";
import { api, ensureOk } from "@/lib/api";
import type { SetupInput } from "@/lib/contracts";
import { setupStatusQuery } from "@/lib/queries";
import SetupForm from "../features/auth/SetupForm.vue";

// First-instance setup (branding and the first administrator). Boot sends
// `/setup` to this Vue page (`src/app-boundary.ts` `/^\/setup\/?$/i`).

const router = useRouter();
const queryClient = useQueryClient();
const setup = useQuery(setupStatusQuery);
const brandingName = computed(() => setup.data.value?.branding.name);
const leaving = computed(() => setup.data.value?.needed === false);

watchEffect(() => {
  if (setup.isLoading.value || setup.isError.value) return;
  if (setup.data.value && !setup.data.value.needed) {
    // /login is already a Vue page: stay in this app.
    router.replace("/login").catch(() => {
      // Recover with a full load if the SPA navigation fails.
      window.location.replace("/login");
    });
  }
});

async function onSubmit(input: SetupInput): Promise<void> {
  await ensureOk(
    await api.POST("/api/v1/setup", {
      body: {
        email: input.email,
        password: input.password,
        givenName: input.givenName,
        familyName: input.familyName || undefined,
        workspaceSlug: input.workspaceSlug,
        workspaceName: input.workspaceName,
      },
    }),
  );
  await queryClient.invalidateQueries();
  // Home is Vue too; a full load starts it with fresh session/workspace queries.
  window.location.replace("/");
}
</script>

<template>
  <p v-if="setup.isLoading.value || leaving" role="status" class="p-8 text-muted">{{
    t("load.loading")
  }}</p>
  <div v-else-if="setup.isError.value" class="p-8">
    <p role="alert" class="text-muted">{{ t("load.failed") }}</p>
    <UButton size="sm" class="mt-2" @click="setup.refetch()">{{ t("load.retry") }}</UButton>
  </div>
  <SetupForm v-else :branding-name="brandingName" :submit-setup="onSubmit" />
</template>
