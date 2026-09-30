<script setup lang="ts">
import { t } from "@fvoci/i18n";
import { computed } from "vue";
import { EMBED_ICON, EMBED_KIND_KEY, type EmbedCardState } from "../embed-model.js";
import type { EmbedEntity } from "../entities.js";

// The embed card (react/blocks.tsx EmbedCardView).
const props = defineProps<{ entity: EmbedEntity; state: EmbedCardState }>();

const kind = computed(() => t(EMBED_KIND_KEY[props.entity]));
const icon = computed(() => {
  if (props.entity === "url") return "";
  return props.state.state === "resolved" && props.state.snapshot.icon
    ? props.state.snapshot.icon
    : EMBED_ICON[props.entity];
});
const refText = computed(() => {
  const state = props.state;
  if (state.state === "plain") return state.ref || t("editor.embed.noRef");
  if (state.state === "loading") return t("editor.embed.loading");
  if (state.state === "inaccessible") return t("editor.embed.inaccessible", { kind: kind.value });
  return state.snapshot.label;
});
const status = computed(() =>
  props.state.state === "resolved" ? props.state.snapshot.status : undefined,
);
</script>

<template>
  <div
    :class="state.state === 'inaccessible' ? 'afn-embed afn-embed-inaccessible' : 'afn-embed'"
    :data-entity="entity"
  >
    <span v-if="icon" class="afn-embed-icon" aria-hidden="true">{{ icon }}</span>
    <span class="afn-embed-kind">{{ kind }}</span>
    <span class="afn-embed-ref">{{ refText }}</span>
    <span v-if="status" class="afn-embed-meta">{{ status }}</span>
  </div>
</template>
