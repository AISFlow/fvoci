<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { ref } from "vue";
import { problemMessage } from "@/lib/api";
import { magicLinkInput, passwordResetInput } from "@/lib/validators";
import AuthAlert from "./AuthAlert.vue";
import AuthField from "./AuthField.vue";
import AuthStatus from "./AuthStatus.vue";
import { useAuthForm } from "./useAuthForm";

const props = defineProps<{
  emailId: string;
  schema: typeof magicLinkInput | typeof passwordResetInput;
  submitLabel: string;
  sentNotice: string;
  errorKey: "error.auth.magic" | "error.auth.resetRequest";
  send: (email: string) => Promise<void>;
}>();

const serverError = ref<string | null>(null);
const sent = ref(false);
const form = useAuthForm({
  initial: { email: "" },
  schema: props.schema,
  ids: { email: props.emailId },
});

const onSubmit = form.handleSubmit(async ({ email }) => {
  serverError.value = null;
  try {
    await props.send(email);
    sent.value = true;
  } catch (err) {
    serverError.value = problemMessage(err, props.errorKey);
  }
});
</script>

<template>
  <form class="auth-shell__stack" novalidate @submit="onSubmit">
    <AuthField
      :id="emailId"
      name="email"
      type="email"
      autocomplete="email"
      :label="t('auth.email')"
      :error="form.errors.email"
      :disabled="form.submitting.value"
      @input="form.onInput('email', $event)"
    />
    <AuthAlert v-if="serverError" :message="serverError" />
    <AuthStatus v-if="sent" :message="sentNotice" />
    <UButton type="submit" size="lg" class="auth-shell__button" :disabled="form.submitting.value">
      {{ form.submitting.value ? t("form.requesting") : submitLabel }}
    </UButton>
  </form>
</template>
