<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { ref } from "vue";
import QueryError from "../../components/QueryError.vue";
import QueryLoading from "../../components/QueryLoading.vue";
import { inputChecked, inputText, useZodForm } from "../../composables/useZodForm";
import type { components } from "@/generated/api";
import { ProblemError } from "@/lib/api";
import { formatDateKo } from "@/lib/datetime";
import { LEGAL_KIND_PRESETS, legalPublishInput } from "@/features/settings/legal-publish";
import { fieldClass, textareaClass } from "./field-classes";
import "@/features/settings/settings-shell.css";

type LegalDocument = components["schemas"]["LegalDocumentOutput"];
type LegalPublishInput = components["schemas"]["LegalPublishBody"];

const props = defineProps<{
  kind: string;
  onKindChange: (kind: string) => void;
  current: LegalDocument | null;
  loading: boolean;
  error: string | null;
  onRetry: () => void;
  onPublish: (input: LegalPublishInput) => Promise<void>;
}>();

const form = useZodForm({
  schema: () => legalPublishInput,
  defaults: () => ({
    kind: props.kind,
    title: "",
    bodyMarkdown: "",
    required: true,
    effectiveAt: "",
  }),
  fieldIds: { title: "legal-title", bodyMarkdown: "legal-body", effectiveAt: "legal-effective-at" },
});
const serverError = ref<string | null>(null);
const fieldError = ref<string | null>(null);
const published = ref(false);

async function submit(): Promise<void> {
  form.values.kind = props.kind;
  fieldError.value = null;
  serverError.value = null;
  published.value = false;
  await form.submit(async (data) => {
    try {
      await props.onPublish(data);
      form.reset();
      form.values.kind = props.kind;
      published.value = true;
    } catch (err) {
      serverError.value = err instanceof ProblemError ? err.title : t("legal.publishError");
    }
  });
  if (Object.keys(form.errors.value).length > 0) {
    fieldError.value = Object.values(form.errors.value)[0] ?? t("form.invalid");
  }
}
</script>

<template>
  <section class="settings-section" aria-labelledby="legal-manage-title">
    <h2 class="settings-section__title text-title" id="legal-manage-title">{{ t("legal.manage") }}</h2>
    <div class="flex flex-col gap-6">
      <div class="flex flex-col gap-1.5">
        <label class="text-sm font-medium" for="legal-kind">{{ t("legal.kind") }}</label>
        <div class="flex flex-wrap gap-2">
          <UButton
            v-for="p in LEGAL_KIND_PRESETS"
            :key="p.kind"
            type="button"
            variant="outline"
            color="neutral"
            size="sm"
            @click="onKindChange(p.kind)"
          >
            {{ p.label }}({{ p.kind }})
          </UButton>
        </div>
        <input id="legal-kind" :class="fieldClass" :value="kind" @input="onKindChange(inputText($event))" />
      </div>

      <div class="flex flex-col gap-1.5 border-t border-default pt-4">
        <p class="text-sm font-medium">{{ t("legal.current") }}</p>
        <QueryLoading v-if="loading" />
        <QueryError v-if="!loading && error" :message="error" @retry="onRetry" />
        <ul v-if="!loading && !error && current" class="text-sm text-muted" :aria-label="t('legal.current')">
          <li>{{ t("legal.document.title") }}: {{ current.title }}</li>
          <li>{{ t("legal.document.version") }}: v{{ current.version }}</li>
          <li>{{ t("legal.effectiveAt") }}: {{ formatDateKo(current.effectiveAt) }}</li>
          <li>{{ t("legal.requiredFlag") }}: {{ current.required ? t("common.required") : t("common.optional") }}</li>
        </ul>
        <p v-if="!loading && !error && !current" class="text-sm text-muted">{{ t("legal.nonePublished") }}</p>
      </div>

      <form class="flex flex-col gap-3 border-t border-default pt-4" novalidate @submit.prevent="submit">
        <p class="text-sm font-medium">{{ t("legal.publishNew") }}</p>
        <div class="flex flex-col gap-1.5">
          <label class="text-sm font-medium" for="legal-title">{{ t("legal.document.title") }}</label>
          <input
            id="legal-title"
            :class="fieldClass"
            :value="form.values.title"
            @input="form.values.title = inputText($event)"
          />
        </div>
        <div class="flex flex-col gap-1.5">
          <label class="text-sm font-medium" for="legal-body">{{ t("legal.bodyMarkdown") }}</label>
          <textarea
            id="legal-body"
            :class="[textareaClass, 'min-h-48']"
            rows="8"
            :value="form.values.bodyMarkdown"
            @input="form.values.bodyMarkdown = inputText($event)"
          />
        </div>
        <div class="flex flex-col gap-1.5">
          <label class="text-sm font-medium" for="legal-effective-at">{{ t("legal.effectiveAt") }}</label>
          <input
            id="legal-effective-at"
            :class="fieldClass"
            type="date"
            :value="form.values.effectiveAt"
            @input="form.values.effectiveAt = inputText($event)"
          />
        </div>
        <div class="flex min-h-11 items-center gap-2">
          <input
            id="legal-required"
            type="checkbox"
            class="size-5"
            :checked="form.values.required === true"
            @change="form.values.required = inputChecked($event)"
          />
          <label class="text-sm font-normal" for="legal-required">{{ t("legal.requiredDoc") }}</label>
        </div>
        <p v-if="fieldError" role="alert" class="text-sm text-error">{{ fieldError }}</p>
        <p v-if="serverError" role="alert" class="text-sm text-error">{{ serverError }}</p>
        <p v-if="published" role="status" class="text-sm text-muted">{{ t("legal.published") }}</p>
        <UButton type="submit" size="sm" class="w-fit" :disabled="form.submitting.value">
          {{ form.submitting.value ? t("form.publishing") : t("legal.publish") }}
        </UButton>
      </form>
    </div>
  </section>
</template>
