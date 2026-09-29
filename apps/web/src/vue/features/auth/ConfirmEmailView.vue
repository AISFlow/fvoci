<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { ref } from "vue";
import { RouterLink } from "vue-router";
import { problemMessage } from "@/lib/api";
import AuthAlert from "./AuthAlert.vue";
import AuthLayout from "./AuthLayout.vue";
import AuthPanel from "./AuthPanel.vue";

const props = defineProps<{
  token: string | null;
  confirmEmail: (token: string) => Promise<void>;
}>();

const pending = ref(false);
const error = ref<string | null>(props.token ? null : t("magic_invalid"));

async function handleClick(): Promise<void> {
  if (!props.token) return;
  pending.value = true;
  error.value = null;
  try {
    await props.confirmEmail(props.token);
  } catch (err) {
    error.value = problemMessage(err, "error.auth.confirm");
  } finally {
    pending.value = false;
  }
}
</script>

<template>
  <AuthLayout>
    <AuthPanel :title="t('confirm.email.title')">
      <div v-if="error" class="auth-shell__stack">
        <AuthAlert :message="error" />
        <RouterLink to="/login" class="auth-shell__link">{{ t("auth.backToLogin") }}</RouterLink>
      </div>
      <UButton
        v-if="token && !error"
        type="button"
        size="lg"
        class="auth-shell__button"
        :disabled="pending"
        @click="handleClick"
      >
        {{ pending ? t("auth.emailChange.confirming") : t("confirm.email.submit") }}
      </UButton>
    </AuthPanel>
  </AuthLayout>
</template>
