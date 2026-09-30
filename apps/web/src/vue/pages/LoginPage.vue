<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useQuery } from "@tanstack/vue-query";
import { computed, ref, watchEffect } from "vue";
import { useRoute } from "vue-router";
import { api, ensureOk } from "@/lib/api";
import { safeReturnTo } from "@/lib/consent";
import { oidcErrorMessage, takeMfaFragment } from "@/lib/oidc";
import { meQuery, providersQuery, setupStatusQuery } from "@/lib/queries";
import { publicInstanceQuery } from "@/lib/queries/instance";
import type { LoginInput } from "@/lib/contracts";
import { redirectTo } from "../session/navigation";
import LoginForm from "../features/auth/LoginForm.vue";
import MfaStep from "../features/auth/MfaStep.vue";

// /login: logout landing, MFA step, and OIDC error query. The boot module
// sends this path to the Vue app (src/app-boundary.ts).

const route = useRoute();
const setup = useQuery(setupStatusQuery);
const instance = useQuery(publicInstanceQuery);
const me = useQuery(meQuery);
const providers = useQuery(providersQuery);

const mfaToken = ref<string | null>(null);
const brandingName = computed(() => setup.data.value?.branding.name);
const operator = computed(() => instance.data.value?.values.operator ?? null);
const returnTo = computed(() =>
  safeReturnTo(
    typeof route.query.returnTo === "string" ? route.query.returnTo : null,
    window.location.origin,
  ),
);
const resetNotice = computed(() => route.query.reset === "1");
const withdrawnNotice = computed(() => route.query.withdrawn === "1");
const notice = computed(() =>
  oidcErrorMessage(typeof route.query.error === "string" ? route.query.error : null),
);

const leaving = computed(() => setup.data.value?.needed === true || me.data.value !== undefined);

watchEffect(() => {
  if (setup.isLoading.value || setup.isError.value) return;
  if (setup.data.value?.needed) {
    redirectTo("/setup");
    return;
  }
  // React SetupGuard mounts LoginPage only after setup succeeds, so #mfa=
  // survives a setup-error refresh. Take the fragment only on that same screen.
  if (mfaToken.value === null) mfaToken.value = takeMfaFragment();
  if (me.data.value) {
    // Start the destination with its own app/query cache after login.
    window.location.replace(returnTo.value);
  }
});

async function enterApp(): Promise<void> {
  if (returnTo.value !== "/") {
    window.location.assign(returnTo.value);
    return;
  }
  window.location.replace("/");
}

async function onLogin(input: LoginInput): Promise<void> {
  const result = await ensureOk(
    await api.POST("/api/v1/auth/login", {
      body: input,
    }),
  );
  if (result.mfaToken) {
    mfaToken.value = result.mfaToken;
    return;
  }
  await enterApp();
}

async function onMagicLink(email: string): Promise<void> {
  await ensureOk(
    await api.POST("/api/v1/auth/magic-link", {
      body: { email },
    }),
  );
}

async function onPasswordReset(email: string): Promise<void> {
  await ensureOk(
    await api.POST("/api/v1/auth/password-reset", {
      body: { email },
    }),
  );
}
</script>

<template>
  <div v-if="setup.isError.value" class="p-8">
    <p role="alert" class="text-muted">{{ t("load.failed") }}</p>
    <UButton size="sm" class="mt-2" @click="setup.refetch()">{{ t("load.retry") }}</UButton>
  </div>
  <p v-else-if="setup.isLoading.value || leaving" role="status" class="p-8 text-muted">{{
    t("load.loading")
  }}</p>
  <MfaStep
    v-else-if="mfaToken !== null"
    :mfa-token="mfaToken"
    :branding-name="brandingName"
    @back="mfaToken = null"
    @verified="enterApp"
  />
  <LoginForm
    v-else
    :branding-name="brandingName"
    :operator="operator"
    :mail-enabled="setup.data.value?.mailEnabled === true"
    :reset-notice="resetNotice"
    :withdrawn-notice="withdrawnNotice"
    :notice="notice"
    :magic-link="providers.data.value?.magicLink"
    :providers="providers.data.value?.providers"
    :providers-loading="providers.isLoading.value"
    :workspace-sso="providers.data.value?.workspaceSso"
    :submit-login="onLogin"
    :send-magic-link="onMagicLink"
    :send-password-reset="onPasswordReset"
  />
</template>
