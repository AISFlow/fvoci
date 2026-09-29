<script setup lang="ts">
import { asSafeHtml, SafeHtml } from "@fvoci/editor/vue";
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { computed, reactive, ref } from "vue";
import { problemMessage } from "@/lib/api";
import { formatDateKo } from "@/lib/datetime";
import type { components } from "@/generated/api";
import AuthAlert from "./AuthAlert.vue";
import AuthLayout from "./AuthLayout.vue";
import AuthPanel from "./AuthPanel.vue";
import AuthStatus from "./AuthStatus.vue";

type LegalDocument = components["schemas"]["LegalDocumentOutput"];

const props = defineProps<{
  pending: LegalDocument[];
  submitConsents: (items: { kind: string; version: number }[]) => Promise<void>;
}>();

const checked = reactive<Record<string, boolean>>({});
const error = ref<string | null>(null);
const submitting = ref(false);

function consentKey(doc: { kind: string; version: number }): string {
  return `${doc.kind}:${doc.version}`;
}

const allChecked = computed(
  () => props.pending.length > 0 && props.pending.every((d) => checked[consentKey(d)] === true),
);

function onSubmit(event: Event): void {
  event.preventDefault();
  if (!allChecked.value || submitting.value) return;
  error.value = null;
  submitting.value = true;
  props
    .submitConsents(props.pending.map((d) => ({ kind: d.kind, version: d.version })))
    .catch((err: unknown) => {
      error.value = problemMessage(err, "error.auth.consent");
    })
    .finally(() => {
      submitting.value = false;
    });
}
</script>

<template>
  <AuthLayout>
    <AuthPanel v-if="pending.length === 0" :title="t('consent.title')">
      <AuthStatus :message="t('consent.empty')" />
    </AuthPanel>
    <AuthPanel v-else :title="t('consent.title')" :lead="t('consent.hint')">
      <div
        v-for="(doc, index) in pending"
        :key="consentKey(doc)"
        class="auth-shell__stack"
        :class="index < pending.length - 1 ? 'auth-shell__divided pb-6' : ''"
      >
        <div>
          <h2 class="auth-shell__text auth-shell__text--strong">{{ doc.title }}</h2>
          <p class="mt-1 auth-shell__dense auth-shell__text--muted">
            {{ t("consent.effectiveAt", { date: formatDateKo(doc.effectiveAt) }) }}
          </p>
        </div>
        <SafeHtml class="auth-shell__doc-body break-keep auth-shell__text" :html="asSafeHtml(doc.bodyHtml)" />
        <div class="auth-shell__check">
          <input
            :id="`consent-${consentKey(doc)}`"
            type="checkbox"
            :checked="checked[consentKey(doc)] ?? false"
            :aria-describedby="`consent-${consentKey(doc)}-title`"
            @change="checked[consentKey(doc)] = ($event.target as HTMLInputElement).checked"
          />
          <label :for="`consent-${consentKey(doc)}`" class="auth-shell__text">
            {{ t("consent.agree") }}
            <span :id="`consent-${consentKey(doc)}-title`" class="sr-only">{{ doc.title }}</span>
          </label>
        </div>
      </div>
      <AuthAlert v-if="error" :message="error" />
      <form novalidate @submit="onSubmit">
        <UButton
          type="submit"
          size="lg"
          class="auth-shell__button"
          :disabled="!allChecked || submitting"
        >
          {{ submitting ? t("form.submitting") : t("consent.submit") }}
        </UButton>
      </form>
    </AuthPanel>
  </AuthLayout>
</template>
