<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UInput from "@nuxt/ui/components/Input.vue";
import UButton from "@nuxt/ui/components/Button.vue";
import { useQuery } from "@tanstack/vue-query";
import { computed, ref, watch } from "vue";
import { ProblemError } from "@/lib/api";
import { failMessage } from "./public-share-fail";
import { publicShareSearchQuery } from "./public-share-search";

const props = defineProps<{ token: string }>();
const emit = defineEmits<{ selectDocument: [id: string]; denied: [error: ProblemError] }>();
const input = ref("");
const q = ref("");
watch(input, (value, _previous, cleanup) => {
  const timer = setTimeout(() => {
    q.value = value;
  }, 300);
  cleanup(() => clearTimeout(timer));
});
watch(
  () => props.token,
  () => {
    input.value = "";
    q.value = "";
  },
  { flush: "sync" },
);
const search = useQuery(() => publicShareSearchQuery(props.token, q.value));
const message = computed(() =>
  search.error.value instanceof ProblemError && search.error.value.status === 429
    ? t("share.search.rateLimited")
    : search.error.value
      ? failMessage(search.error.value)
      : null,
);
watch(search.error, (error) => {
  if (error instanceof ProblemError && error.status === 404) emit("denied", error);
});
// Hide old results during the debounce, as soon as the visible text changes.
const current = computed(() => input.value === q.value && q.value.trim().length > 0);
</script>

<template>
  <section class="flex flex-col gap-3" :aria-label="t('share.search.title')">
    <UInput
      v-model="input"
      type="search"
      :maxlength="200"
      :aria-label="t('share.search.title')"
      :placeholder="t('share.search.title')"
    />
    <p v-if="current && search.isFetching.value" role="status">{{ t("share.search.loading") }}</p>
    <p v-else-if="current && message" role="alert" class="share-page__alert">{{ message }}</p>
    <ul
      v-else-if="current && search.isSuccess.value"
      class="flex flex-col gap-2"
      data-testid="share-search-results"
    >
      <li v-if="search.data.value?.items.length === 0">{{ t("share.search.empty") }}</li>
      <li
        v-for="item in search.data.value?.items ?? []"
        :key="`${item.type}:${item.id}`"
        class="rounded-lg border border-default p-3"
      >
        <UButton
          v-if="item.type === 'document'"
          color="neutral"
          variant="link"
          @click="emit('selectDocument', item.id)"
          >{{ item.title }}</UButton
        >
        <p v-else class="font-medium"
          >{{ item.title }} <span class="text-sm text-muted">{{ t("task.type.task") }}</span></p
        >
        <p v-if="item.snippet" class="text-sm text-muted">
          <template v-for="(piece, index) in item.snippet" :key="index"
            ><mark v-if="piece.match">{{ piece.text }}</mark
            ><span v-else>{{ piece.text }}</span></template
          >
        </p>
      </li>
    </ul>
  </section>
</template>
