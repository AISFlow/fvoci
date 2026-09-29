<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, watchEffect } from "vue";
import { useRoute } from "vue-router";
import { api, ensureOk, ProblemError } from "@/lib/api";
import { erasureRecoveryHash } from "@/lib/erasure-hash";
import { oidcErrorMessage } from "@/lib/oidc";
import { identitiesQuery, meQuery, mfaStatusQuery, providersQuery, setupStatusQuery } from "@/lib/queries";
import LegalNav from "../components/LegalNav.vue";
import AccountSettingsView from "../features/settings/AccountSettingsView.vue";
import { loginPath, redirectTo } from "../session/navigation";
import "@/features/settings/settings-shell.css";

// Coordinator-owned src/app-boundary.ts still sends this path to React.
// When accepting: /^\/settings\/account\/?$/i plus VUE_ROUTE_PATHS.accountSettings.

const route = useRoute();
const queryClient = useQueryClient();
const setup = useQuery(setupStatusQuery);
const me = useQuery(meQuery);
const identities = useQuery(identitiesQuery);
const providers = useQuery(providersQuery);
const mfa = useQuery(mfaStatusQuery);

watchEffect(() => {
  if (setup.data.value?.needed) redirectTo("/setup");
  else if (me.error.value instanceof ProblemError && me.error.value.status === 401) {
    redirectTo(loginPath(window.location));
  }
});

const successNotice = computed(() => {
  if (route.query.linked === "1") return t("auth.account.linkedNotice");
  if (route.query.email_changed === "1") return t("auth.account.emailChangedNotice");
  return null;
});
const errorNotice = computed(() =>
  oidcErrorMessage(typeof route.query.error === "string" ? route.query.error : null),
);

const failed = computed(() => identities.isError.value || providers.isError.value || mfa.isError.value);
const ready = computed(
  () => me.data.value !== undefined && identities.data.value && providers.data.value && mfa.data.value,
);

function retry(): void {
  if (identities.isError.value) void identities.refetch();
  if (providers.isError.value) void providers.refetch();
  if (mfa.isError.value) void mfa.refetch();
}

async function downloadMeExport(): Promise<void> {
  const blob = await ensureOk(await api.GET("/api/v1/me/export", { parseAs: "blob" }));
  const href = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = href;
  link.download = "fvoci-export.zip";
  link.rel = "noopener";
  document.body.appendChild(link);
  link.click();
  link.remove();
  URL.revokeObjectURL(href);
}

async function completeWithdraw(input: {
  currentPassword: string | null;
  emailLocalPart: string | null;
}): Promise<void> {
  const result = await ensureOk(await api.POST("/api/v1/auth/withdraw", { body: input }));
  window.location.assign(
    `/cancel-withdraw#${erasureRecoveryHash({
      token: result.cancelToken,
      eraseAt: result.eraseAt,
      mailSent: result.mailSent,
    })}`,
  );
}
</script>

<template>
  <div class="flex min-h-screen flex-col">
    <header class="flex flex-wrap items-center justify-between gap-3 border-b border-default px-4 py-3">
      <a href="/" class="underline underline-offset-2">{{ t("nav.backHome") }}</a>
    </header>
    <main class="flex-1 p-4">
      <div class="settings-page">
        <div v-if="failed">
          <p role="alert" class="text-muted">{{ t("load.failed") }}</p>
          <UButton type="button" size="sm" class="mt-2" @click="retry">{{ t("load.retry") }}</UButton>
        </div>
        <p v-else-if="!ready" role="status" class="text-muted">{{ t("load.loading") }}</p>
        <AccountSettingsView
          v-else-if="me.data.value && identities.data.value && providers.data.value && mfa.data.value"
          :me="me.data.value"
          :identities="identities.data.value.items"
          :providers="providers.data.value.providers"
          :magic-link="providers.data.value.magicLink"
          :success-notice="successNotice"
          :error-notice="errorNotice"
          :mfa-status="mfa.data.value"
          :on-save-name="
            async (input) => {
              await ensureOk(await api.PATCH('/api/v1/auth/me', { body: input }));
              await queryClient.invalidateQueries({ queryKey: ['auth', 'me'] });
            }
          "
          :on-send-verification="
            async (email) => {
              await ensureOk(await api.POST('/api/v1/auth/magic-link', { body: { email } }));
            }
          "
          :on-change-email="
            async (newEmail) => {
              await ensureOk(await api.PATCH('/api/v1/auth/email', { body: { newEmail } }));
            }
          "
          :on-change-password="
            async (input) => {
              await ensureOk(await api.PATCH('/api/v1/auth/password', { body: input }));
              await queryClient.invalidateQueries({ queryKey: ['auth', 'me'] });
            }
          "
          :on-withdraw="completeWithdraw"
          :on-export="downloadMeExport"
          :on-unlink="
            async (provider) => {
              await ensureOk(
                await api.POST('/api/v1/auth/oidc/{provider}/unlink', { params: { path: { provider } } }),
              );
              await queryClient.invalidateQueries({ queryKey: identitiesQuery.queryKey });
            }
          "
          :on-mfa-setup="async (input) => ensureOk(await api.POST('/api/v1/auth/mfa/setup', { body: input }))"
          :on-mfa-enable="
            async (code) => {
              await ensureOk(await api.POST('/api/v1/auth/mfa/enable', { body: { code } }));
              await queryClient.invalidateQueries({ queryKey: mfaStatusQuery.queryKey });
            }
          "
          :on-mfa-disable="
            async (input) => {
              await ensureOk(await api.POST('/api/v1/auth/mfa/disable', { body: input }));
              await queryClient.invalidateQueries({ queryKey: mfaStatusQuery.queryKey });
            }
          "
        />
      </div>
    </main>
    <footer class="border-t border-default px-4 py-3">
      <LegalNav />
    </footer>
  </div>
</template>
