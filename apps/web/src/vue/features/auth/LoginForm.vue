<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { ref } from "vue";
import { problemMessage } from "@/lib/api";
import type { LoginInput, ProviderOutput } from "@/lib/contracts";
import { oidcStartHref } from "@/lib/oidc";
import { loginInput, magicLinkInput, passwordResetInput } from "@/lib/validators";
import type { OperatorInfo } from "@/features/legal/operator-fields";
import AuthAlert from "./AuthAlert.vue";
import AuthDisclosure from "./AuthDisclosure.vue";
import AuthField from "./AuthField.vue";
import AuthLayout from "./AuthLayout.vue";
import AuthPanel from "./AuthPanel.vue";
import AuthStatus from "./AuthStatus.vue";
import EmailActionForm from "./EmailActionForm.vue";
import SsoSlugForm from "./SsoSlugForm.vue";
import { useAuthForm } from "./useAuthForm";

const RESET_SENT_NOTICE = t("auth.reset.sent");
const RESET_DONE_NOTICE = t("auth.reset.done");
const MAGIC_SENT_NOTICE = t("auth.magic.sent");
const MAGIC_DISABLED_NOTICE = t("auth.magic.disabled");
const WITHDRAWN_NOTICE = t("auth.withdrawn");

const props = defineProps<{
  submitLogin: (input: LoginInput) => Promise<void>;
  brandingName?: string | null;
  operator?: OperatorInfo | null;
  unavailableNotice?: string | null;
  mailEnabled?: boolean;
  sendPasswordReset?: (email: string) => Promise<void>;
  resetNotice?: boolean;
  withdrawnNotice?: boolean;
  // `undefined` while GET /auth/providers is loading: show neither state.
  magicLink?: boolean;
  sendMagicLink?: (email: string) => Promise<void>;
  // OIDC callback `?error=` message.
  notice?: string | null;
  providers?: ProviderOutput[];
  providersLoading?: boolean;
  workspaceSso?: boolean;
}>();

const serverError = ref<string | null>(null);
const magicOpen = ref(false);
const resetOpen = ref(false);
const form = useAuthForm({
  initial: { email: "", password: "" },
  schema: loginInput,
  ids: { email: "login-email", password: "login-password" },
});

const onSubmit = form.handleSubmit(async (values) => {
  serverError.value = null;
  try {
    await props.submitLogin(values);
  } catch (err) {
    serverError.value = problemMessage(err, "error.auth.login");
  }
});
</script>

<template>
  <AuthLayout :branding-name="brandingName" :footer-operator="operator ?? null">
    <AuthPanel :title="t('auth.login')">
      <AuthStatus v-if="resetNotice" :message="RESET_DONE_NOTICE" />
      <AuthStatus v-if="withdrawnNotice" :message="WITHDRAWN_NOTICE" />
      <AuthStatus v-if="unavailableNotice" :message="unavailableNotice ?? ''" />
      <AuthAlert v-if="notice" :message="notice ?? ''" />
      <form class="auth-shell__stack auth-shell__stack--form" novalidate @submit="onSubmit">
        <AuthField
          id="login-email"
          name="email"
          type="email"
          autocomplete="email"
          :label="t('auth.email')"
          :error="form.errors.email"
          :disabled="form.submitting.value"
          @input="form.onInput('email', $event)"
        />
        <AuthField
          id="login-password"
          name="password"
          type="password"
          autocomplete="current-password"
          :label="t('auth.password')"
          :error="form.errors.password"
          :disabled="form.submitting.value"
          @input="form.onInput('password', $event)"
        />
        <AuthAlert v-if="serverError" :message="serverError" />
        <UButton
          type="submit"
          size="lg"
          class="auth-shell__button"
          :disabled="form.submitting.value"
        >
          {{ form.submitting.value ? t("auth.login.pending") : t("auth.login") }}
        </UButton>
      </form>
      <template v-if="magicLink === true && sendMagicLink">
        <hr class="auth-shell__rule" />
        <AuthDisclosure
          :trigger="t('auth.magic.cta')"
          :open="magicOpen"
          @update:open="magicOpen = $event"
        >
          <EmailActionForm
            email-id="magic-link-email"
            :schema="magicLinkInput"
            :send="sendMagicLink"
            :submit-label="t('auth.magic.submit')"
            :sent-notice="MAGIC_SENT_NOTICE"
            error-key="error.auth.magic"
          />
        </AuthDisclosure>
      </template>
      <AuthDisclosure
        v-if="mailEnabled && sendPasswordReset"
        :trigger="t('auth.reset.forgot')"
        :open="resetOpen"
        @update:open="resetOpen = $event"
      >
        <EmailActionForm
          email-id="password-reset-email"
          :schema="passwordResetInput"
          :send="sendPasswordReset"
          :submit-label="t('auth.reset.submit')"
          :sent-notice="RESET_SENT_NOTICE"
          error-key="error.auth.resetRequest"
        />
      </AuthDisclosure>
      <AuthStatus v-if="magicLink === false" :message="MAGIC_DISABLED_NOTICE" />
      <template v-if="providers && providers.length > 0">
        <hr class="auth-shell__rule" />
        <p class="auth-shell__text auth-shell__text--strong auth-shell__text--muted">{{
          t("auth.login.social")
        }}</p>
        <div class="auth-shell__stack">
          <a
            v-for="p in providers"
            :key="p.provider"
            :href="oidcStartHref(p.provider)"
            class="auth-shell__outline-link"
          >
            {{ p.label }}
          </a>
        </div>
      </template>
      <template v-if="!providersLoading && workspaceSso === true">
        <hr class="auth-shell__rule" />
        <SsoSlugForm />
      </template>
    </AuthPanel>
  </AuthLayout>
</template>
