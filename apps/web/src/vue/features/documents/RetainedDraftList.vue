<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { ownerKey, type RetainedOffDraft } from "@/features/documents/off-wiki-draft-slot";
import { useRetainedDraftExport } from "../../composables/useOffWikiBodySlots";

// Retained OFF drafts of targets the page can no longer reach. Local copy and
// download only: nothing here can send a draft to the server.
defineProps<{ drafts: readonly RetainedOffDraft[] }>();
const { note, run } = useRetainedDraftExport();
</script>

<template>
  <section v-if="drafts.length > 0" data-testid="off-draft-retained-list">
    <p class="break-keep">{{ t("doc.off.retainedList") }}</p>
    <div
      v-for="draft in drafts"
      :key="ownerKey(draft.owner)"
      class="mt-2 flex flex-wrap items-center gap-2"
      data-testid="off-draft-retained"
    >
      <span class="min-w-0 break-all text-muted">{{ draft.excerpt }}</span>
      <UButton size="sm" @click="run(draft, 'copy')">{{ t("doc.off.copyDraft") }}</UButton>
      <UButton size="sm" @click="run(draft, 'download')">{{ t("doc.off.downloadDraft") }}</UButton>
      <p v-if="draft.unstored" role="alert" class="w-full break-keep">{{
        t("doc.off.retainedRowUnstored")
      }}</p>
    </div>
    <p v-if="note" role="status" class="mt-2">{{ note }}</p>
  </section>
</template>
