<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { ref } from "vue";
import { RouterLink } from "vue-router";
import { FALLBACK_TZ } from "@/lib/datetime";
import type { ErasureFragment } from "@/lib/erasure-hash";
import AuthAlert from "./AuthAlert.vue";
import AuthLayout from "./AuthLayout.vue";
import AuthPanel from "./AuthPanel.vue";
import AuthStatus from "./AuthStatus.vue";

const props = defineProps<
  ErasureFragment & {
    recoveryHref: string | null;
    submitCancel: (token: string) => Promise<void>;
  }
>();

const pending = ref(false);
const done = ref(false);
const copied = ref(false);
const error = ref<string | null>(props.token ? null : t("auth.erasure.cancel.failed"));

function formatDeadline(iso: string): string {
  return new Date(iso).toLocaleString("ko-KR", {
    hour12: false,
    timeZone: FALLBACK_TZ,
    year: "numeric",
    month: "long",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

async function copyText(value: string): Promise<void> {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(value);
    return;
  }
  throw new Error("clipboard unavailable");
}

async function handleCancel(): Promise<void> {
  if (!props.token || pending.value) return;
  pending.value = true;
  error.value = null;
  try {
    await props.submitCancel(props.token);
    done.value = true;
  } catch {
    error.value = t("auth.erasure.cancel.failed");
  } finally {
    pending.value = false;
  }
}

function handleCopy(): void {
  if (!props.recoveryHref) return;
  void copyText(props.recoveryHref).then(
    () => {
      copied.value = true;
    },
    () => {
      copied.value = false;
    },
  );
}
</script>

<template>
  <AuthLayout>
    <AuthPanel :title="t('auth.erasure.title')">
      <div v-if="done" class="auth-shell__stack">
        <AuthStatus :message="t('auth.erasure.cancel.done')" />
        <RouterLink to="/login" class="auth-shell__link">{{ t("auth.backToLogin") }}</RouterLink>
      </div>
      <div v-else class="auth-shell__stack auth-shell__stack--form">
        <AuthStatus
          v-if="scheduled && eraseAt"
          :message="t('auth.erasure.scheduled', { date: formatDeadline(eraseAt) })"
        />
        <template v-if="scheduled && recoveryHref">
          <AuthStatus
            :message="
              mailSent === false ? t('auth.erasure.mailNotSent') : t('auth.erasure.copyHint')
            "
          />
          <p class="break-all font-mono auth-shell__text">{{ recoveryHref }}</p>
          <UButton
            type="button"
            variant="outline"
            size="lg"
            color="neutral"
            class="auth-shell__button"
            @click="handleCopy"
          >
            {{ copied ? t("common.copyLink.done") : t("common.copyLink") }}
          </UButton>
        </template>
        <div v-if="error" class="auth-shell__stack">
          <AuthAlert :message="error" />
          <RouterLink to="/login" class="auth-shell__link">{{ t("auth.backToLogin") }}</RouterLink>
        </div>
        <UButton
          v-if="token"
          type="button"
          size="lg"
          class="auth-shell__button"
          :disabled="pending"
          @click="handleCancel"
        >
          {{ pending ? t("auth.erasure.cancel.pending") : t("auth.erasure.cancel") }}
        </UButton>
      </div>
    </AuthPanel>
  </AuthLayout>
</template>
