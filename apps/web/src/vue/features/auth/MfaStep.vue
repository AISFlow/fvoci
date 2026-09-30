<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { ref } from "vue";
import { api, ensureOk, problemMessage } from "@/lib/api";
import AuthAlert from "./AuthAlert.vue";
import AuthField from "./AuthField.vue";
import AuthLayout from "./AuthLayout.vue";
import AuthPanel from "./AuthPanel.vue";
import { useAuthForm } from "./useAuthForm";

const props = defineProps<{
  mfaToken: string;
  brandingName?: string | null;
}>();
const emit = defineEmits<{
  back: [];
  verified: [];
}>();

const serverError = ref<string | null>(null);
const form = useAuthForm({
  initial: { code: "" },
  ids: { code: "mfa-code" },
});

const onSubmit = form.handleSubmit(async ({ code }) => {
  serverError.value = null;
  const trimmed = code.trim();
  if (trimmed === "") {
    form.setError("code", t("form.too_small"));
    return;
  }
  try {
    await ensureOk(
      await api.POST("/api/v1/auth/mfa/verify", {
        body: { mfaToken: props.mfaToken, code: trimmed },
      }),
    );
    emit("verified");
  } catch (err) {
    serverError.value = problemMessage(err, "error.auth.mfa");
  }
});
</script>

<template>
  <AuthLayout :branding-name="brandingName">
    <AuthPanel :title="t('auth.mfa.title')">
      <form class="auth-shell__stack auth-shell__stack--form" novalidate @submit="onSubmit">
        <AuthField
          id="mfa-code"
          name="code"
          autocomplete="one-time-code"
          inputmode="numeric"
          autofocus
          :label="t('auth.mfa.code')"
          :hint="t('auth.mfa.code.hint')"
          :error="form.errors.code"
          :disabled="form.submitting.value"
          @input="form.onInput('code', $event)"
        />
        <AuthAlert v-if="serverError" :message="serverError" />
        <UButton
          type="submit"
          size="lg"
          class="auth-shell__button"
          :disabled="form.submitting.value"
        >
          {{ form.submitting.value ? t("auth.mfa.verifying") : t("auth.mfa.verify") }}
        </UButton>
        <UButton
          type="button"
          variant="link"
          class="auth-shell__link-button auth-shell__button"
          @click="emit('back')"
        >
          {{ t("auth.mfa.back") }}
        </UButton>
      </form>
    </AuthPanel>
  </AuthLayout>
</template>
