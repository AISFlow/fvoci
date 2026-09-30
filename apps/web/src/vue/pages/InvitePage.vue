<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useQuery } from "@tanstack/vue-query";
import { computed, ref, watchEffect } from "vue";
import { useRoute } from "vue-router";
import { api, ensureOk, ProblemError } from "@/lib/api";
import type { InvitationAcceptInput } from "@/lib/contracts";
import { invitationPublicQuery, providersQuery, setupStatusQuery } from "@/lib/queries";
import { redirectTo } from "../session/navigation";
import AuthAlert from "../features/auth/AuthAlert.vue";
import AuthLayout from "../features/auth/AuthLayout.vue";
import AuthPanel from "../features/auth/AuthPanel.vue";
import InviteAcceptForm from "../features/auth/InviteAcceptForm.vue";
import MfaStep from "../features/auth/MfaStep.vue";

// /invite/:token: public invitation (consents, password accept, OIDC start).

const route = useRoute();
const token = computed(() => String(route.params.token ?? ""));
const setup = useQuery(setupStatusQuery);
// React SetupGuard mounts InvitePage only after setup succeeds.
const setupReady = computed(
  () =>
    !setup.isLoading.value &&
    !setup.isError.value &&
    setup.data.value?.needed !== true,
);
const invitation = useQuery(() => ({
  ...invitationPublicQuery(token.value),
  enabled: setupReady.value,
}));
const providers = useQuery(() => ({
  ...providersQuery,
  enabled: setupReady.value,
}));

const mfaToken = ref<string | null>(null);
const brandingName = computed(() => setup.data.value?.branding.name);
const leaving = computed(() => setup.data.value?.needed === true);

watchEffect(() => {
  if (setup.isLoading.value || setup.isError.value) return;
  if (setup.data.value?.needed) {
    redirectTo("/setup");
  }
});

async function enterApp(): Promise<void> {
  window.location.assign("/");
}

function leaveToLogin(): void {
  window.location.assign("/login");
}

async function onAccept(input: InvitationAcceptInput): Promise<void> {
  const result = await ensureOk(
    await api.POST("/api/v1/invitations/{token}/accept", {
      params: { path: { token: token.value } },
      body: input,
    }),
  );
  if (result.mfaToken) {
    mfaToken.value = result.mfaToken;
    return;
  }
  await enterApp();
}

const loadError = computed(() => {
  const err = invitation.error.value;
  if (err instanceof ProblemError) return err.title;
  return t("auth.invite.failed");
});
</script>

<template>
  <p v-if="setup.isLoading.value || leaving" role="status" class="p-8 text-muted">{{ t("load.loading") }}</p>
  <div v-else-if="setup.isError.value" class="p-8">
    <p role="alert" class="text-muted">{{ t("load.failed") }}</p>
    <UButton size="sm" class="mt-2" @click="setup.refetch()">{{ t("load.retry") }}</UButton>
  </div>
  <MfaStep
    v-else-if="mfaToken !== null"
    :mfa-token="mfaToken"
    :branding-name="brandingName"
    @back="leaveToLogin"
    @verified="enterApp"
  />
  <AuthLayout v-else-if="invitation.isError.value">
    <AuthPanel :title="t('auth.invite.title', { name: '…' })">
      <AuthAlert :message="loadError" />
    </AuthPanel>
  </AuthLayout>
  <p v-else-if="!invitation.data.value" role="status" class="p-8 text-muted">{{ t("load.loading") }}</p>
  <InviteAcceptForm
    v-else
    :invitation="invitation.data.value"
    :token="token"
    :branding-name="brandingName"
    :providers="providers.data.value?.providers"
    :submit-accept="onAccept"
  />
</template>
