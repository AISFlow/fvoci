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

// First-instance setup (branding and the first administrator). Boot still
// sends this path to the React app until apps/web/src/app-boundary.ts includes:
//   /^\/setup\/?$/i
// Pair that with VUE_ROUTE_PATHS.setup = "/setup" (app-boundary.test.ts).

const router = useRouter();
const queryClient = useQueryClient();
const setup = useQuery(setupStatusQuery);
const brandingName = computed(() => setup.data.value?.branding.name);
const leaving = computed(() => setup.data.value?.needed === false);

watchEffect(() => {
  if (setup.isLoading.value || setup.isError.value) return;
  if (setup.data.value && !setup.data.value.needed) {
    // /login is already a Vue page: stay in this app.
    void router.replace("/login");
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
  // Home is the React app: a full load.
  window.location.replace("/");
}
</script>

<template>
  <p v-if="setup.isLoading.value || leaving" role="status" class="p-8 text-muted">{{ t("load.loading") }}</p>
  <div v-else-if="setup.isError.value" class="p-8">
    <p role="alert" class="text-muted">{{ t("load.failed") }}</p>
    <UButton size="sm" class="mt-2" @click="setup.refetch()">{{ t("load.retry") }}</UButton>
  </div>
  <SetupForm v-else :branding-name="brandingName" :submit-setup="onSubmit" />
</template>
