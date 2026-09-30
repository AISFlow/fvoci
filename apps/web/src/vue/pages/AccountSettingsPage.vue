<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, watch, watchEffect } from "vue";
import { useRoute } from "vue-router";
import {
  changePassword,
  disableMfa,
  downloadMeExport,
  enableMfa,
  requestEmailChange,
  saveProfileName,
  sendEmailVerification,
  setUpMfa,
  unlinkIdentity,
  withdrawAccount,
} from "@/features/settings/account-requests";
import { oidcErrorMessage } from "@/lib/oidc";
import { api, ensureOk, loadErrorMessage, ProblemError } from "@/lib/api";
import { applyTextScale } from "@/lib/ui-preferences";
import { identitiesQuery, meQuery, mfaStatusQuery, providersQuery } from "@/lib/queries";
import QueryLoading from "../components/QueryLoading.vue";
import AccountSettingsView from "../features/settings/AccountSettingsView.vue";
import MfaSection from "../features/settings/MfaSection.vue";
import AccountTokensSection from "../features/settings/AccountTokensSection.vue";
import { loginPath, redirectTo } from "../session/navigation";
import "@/features/settings/settings-shell.css";

const queryClient = useQueryClient();
const route = useRoute();
const me = useQuery(meQuery);
const identities = useQuery(identitiesQuery);
const providers = useQuery(providersQuery);
const mfa = useQuery(mfaStatusQuery);
watch(
  () => me.data.value?.textScale,
  (scale) => {
    if (scale !== undefined) applyTextScale(scale);
  },
  { immediate: true },
);

function queryString(value: unknown): string | null {
  return typeof value === "string" ? value : null;
}

const successNotice = computed(() => {
  if (queryString(route.query.linked) === "1") return t("auth.account.linkedNotice");
  if (queryString(route.query.email_changed) === "1") return t("auth.account.emailChangedNotice");
  return null;
});
const errorNotice = computed(() => oidcErrorMessage(queryString(route.query.error)));

watchEffect(() => {
  if (me.error.value instanceof ProblemError && me.error.value.status === 401)
    redirectTo(loginPath(window.location));
});

const failed = computed(
  () =>
    me.isError.value || identities.isError.value || providers.isError.value || mfa.isError.value,
);
const failure = computed(() =>
  loadErrorMessage(
    me.error.value ?? identities.error.value ?? providers.error.value ?? mfa.error.value,
  ),
);
const ready = computed(
  () => me.data.value && identities.data.value && providers.data.value && mfa.data.value,
);

async function retry(): Promise<void> {
  await Promise.all([me.refetch(), identities.refetch(), providers.refetch(), mfa.refetch()]);
}

async function onWithdraw(input: Parameters<typeof withdrawAccount>[0]): Promise<void> {
  window.location.assign(await withdrawAccount(input));
}

async function savePreferences(input: {
  locale: "ko";
  timezone: string;
  weekStartsOn: number;
  textScale: number;
}): Promise<void> {
  const current = me.data.value;
  if (!current) return;
  const committed = await ensureOk(
    await api.PATCH("/api/v1/auth/me", {
      body: { givenName: current.givenName, ...input },
    }),
  );
  queryClient.setQueryData(meQuery.queryKey, committed);
  await queryClient.invalidateQueries({ queryKey: meQuery.queryKey });
}
</script>

<template>
  <div class="flex min-h-screen flex-col">
    <header class="border-b border-default px-4 py-3">
      <a href="/" class="underline underline-offset-2">{{ t("nav.backHome") }}</a>
    </header>
    <main class="flex-1 p-4">
      <div class="settings-page">
        <div v-if="failed">
          <p role="alert" class="text-muted">{{ failure }}</p>
          <UButton type="button" size="sm" class="mt-2" @click="retry">{{
            t("load.retry")
          }}</UButton>
        </div>
        <QueryLoading v-else-if="!ready" />
        <AccountSettingsView
          v-else-if="
            me.data.value && identities.data.value && providers.data.value && mfa.data.value
          "
          :me="me.data.value"
          :identities="identities.data.value.items"
          :providers="providers.data.value.providers"
          :magic-link="providers.data.value.magicLink"
          :success-notice="successNotice"
          :error-notice="errorNotice"
          :on-save-name="(input) => saveProfileName(queryClient, input)"
          :on-save-preferences="savePreferences"
          :on-send-verification="sendEmailVerification"
          :on-change-email="requestEmailChange"
          :on-change-password="(input) => changePassword(queryClient, input)"
          :on-withdraw="onWithdraw"
          :on-export="downloadMeExport"
          :on-unlink="(provider) => unlinkIdentity(queryClient, provider)"
        >
          <template #mfa>
            <MfaSection
              :status="mfa.data.value"
              :has-password="me.data.value.hasPassword"
              :on-setup="setUpMfa"
              :on-enable="(code) => enableMfa(queryClient, code)"
              :on-disable="(input) => disableMfa(queryClient, input)"
            />
          </template>
        </AccountSettingsView>
        <AccountTokensSection
          v-if="ready && !failed && me.data.value"
          :timezone="me.data.value.timezone"
          class="mt-6"
        />
      </div>
    </main>
    <footer class="border-t border-default px-4 py-3">
      <nav
        class="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted"
        :aria-label="t('operator.title')"
      >
        <a href="/service-info" class="underline underline-offset-2">{{ t("operator.title") }}</a>
        <a href="/legal/terms" class="underline underline-offset-2">{{ t("legal.terms") }}</a>
        <a href="/legal/privacy" class="underline underline-offset-2">{{ t("legal.privacy") }}</a>
      </nav>
    </footer>
  </div>
</template>
