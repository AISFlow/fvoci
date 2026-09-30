<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { ref } from "vue";
import { RouterLink } from "vue-router";
import { problemMessage } from "@/lib/api";
import { passwordResetConfirmInput } from "@/lib/validators";
import AuthAlert from "./AuthAlert.vue";
import AuthField from "./AuthField.vue";
import AuthLayout from "./AuthLayout.vue";
import AuthPanel from "./AuthPanel.vue";
import { useAuthForm } from "./useAuthForm";

const props = defineProps<{
  token: string | null;
  submitConfirm: (newPassword: string) => Promise<void>;
}>();

const serverError = ref<string | null>(props.token ? null : t("magic_invalid"));
const form = useAuthForm({
  initial: { newPassword: "" },
  schema: passwordResetConfirmInput.omit({ token: true }),
  ids: { newPassword: "reset-password-new" },
});

const onSubmit = form.handleSubmit(async ({ newPassword }) => {
  serverError.value = null;
  try {
    await props.submitConfirm(newPassword);
  } catch (err) {
    serverError.value = problemMessage(err, "error.password.change");
  }
});
</script>

<template>
  <AuthLayout>
    <AuthPanel :title="t('auth.reset.title')">
      <template v-if="!token">
        <AuthAlert :message="serverError ?? t('magic_invalid')" />
        <RouterLink to="/login" class="auth-shell__link">{{ t("auth.reset.retry") }}</RouterLink>
      </template>
      <form v-else class="auth-shell__stack auth-shell__stack--form" novalidate @submit="onSubmit">
        <AuthField
          id="reset-password-new"
          name="newPassword"
          type="password"
          autocomplete="new-password"
          :label="t('auth.passwordNew')"
          :error="form.errors.newPassword"
          :disabled="form.submitting.value"
          @input="form.onInput('newPassword', $event)"
        />
        <div v-if="serverError" class="auth-shell__stack">
          <AuthAlert :message="serverError" />
          <RouterLink to="/login" class="auth-shell__link">{{ t("auth.reset.retry") }}</RouterLink>
        </div>
        <UButton type="submit" size="lg" class="auth-shell__button" :disabled="form.submitting.value">
          {{ form.submitting.value ? t("form.changing") : t("auth.reset.change") }}
        </UButton>
      </form>
    </AuthPanel>
  </AuthLayout>
</template>
