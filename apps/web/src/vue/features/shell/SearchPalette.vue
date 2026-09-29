<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import UInput from "@nuxt/ui/components/Input.vue";
import { useId, watch } from "vue";
import { useRoute } from "vue-router";
import { searchPath } from "@/lib/href";
import NativeModal from "../../components/NativeModal.vue";
import SearchResultList from "./SearchResultList.vue";
import { useSearchPalette } from "./useSearchPalette";

// The header search palette (features/workspace/search-command.tsx): Ctrl+K
// or Cmd+K or the button opens it, results follow the typing, Enter or "see
// all" opens the search page (a React page, full load). A modal <dialog>:
// Escape or a click on the backdrop closes it and focus returns.
const props = defineProps<{ slug: string; workspaceId: string }>();
const dialogId = useId();
const { open, draft, q, results, items } = useSearchPalette({
  workspaceId: () => props.workspaceId,
  keyTarget: window,
});
const { isFetching: searching, isError: failed } = results;

// A result in this app (a wiki document) is an in-app navigation that may
// keep this shell mounted: the palette closes with it.
const route = useRoute();
watch(
  () => route.fullPath,
  () => {
    open.value = false;
  },
);

function submit(): void {
  const next = draft.value.trim();
  if (!next) return;
  open.value = false;
  window.location.assign(searchPath(props.slug, { q: next }));
}
</script>

<template>
  <UButton
    size="sm"
    variant="outline"
    color="neutral"
    aria-haspopup="dialog"
    :aria-expanded="open"
    :aria-controls="open ? dialogId : undefined"
    @click="open = true"
  >
    {{ t("nav.search") }}
  </UButton>
  <NativeModal
    :id="dialogId"
    :open="open"
    :labelled-by="`${dialogId}-title`"
    dialog-class="mx-auto mt-[12vh] w-[min(36rem,calc(100%-2rem))] max-h-[76dvh] overflow-auto rounded-xl border border-default bg-default p-0 text-default shadow-xl backdrop:bg-black/30"
    close-on-backdrop
    @close="open = false"
  >
    <div class="flex flex-col gap-3 p-4">
      <h2 :id="`${dialogId}-title`" class="m-0 text-lg font-semibold break-keep">{{ t("search.command") }}</h2>
      <form class="flex gap-2" @submit.prevent="submit">
        <label class="sr-only" :for="`${dialogId}-q`">{{ t("search.query") }}</label>
        <UInput
          :id="`${dialogId}-q`"
          v-model="draft"
          class="w-full"
          autofocus
          :placeholder="t('search.queryPlaceholder')"
          autocomplete="off"
          enterkeyhint="search"
        />
      </form>
      <p v-if="searching" role="status" class="m-0 text-sm text-muted">{{ t("search.loading") }}</p>
      <p v-if="failed" role="alert" class="m-0 text-sm text-muted">{{ t("search.failed") }}</p>
      <p v-if="!searching && !failed && q && items.length === 0" class="m-0 text-sm text-muted">
        {{ t("search.empty") }}
      </p>
      <SearchResultList v-if="items.length > 0" :slug="slug" :items="items.slice(0, 8)" />
      <a v-if="q" :href="searchPath(slug, { q })" class="w-fit text-sm underline underline-offset-2">{{
        t("search.seeAll")
      }}</a>
      <p v-else class="m-0 text-sm text-muted break-keep">{{ t("search.hint") }}</p>
    </div>
  </NativeModal>
</template>
