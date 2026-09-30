<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import type { CollabPeer } from "@/features/documents/collab-model";

// Peers in the room, one per tab (features/documents/collab-presence.tsx).
defineProps<{ peers: readonly CollabPeer[] }>();
const emit = defineEmits<{ jump: [blockId: string] }>();
</script>

<template>
  <ul
    v-if="peers.length > 0"
    class="document-page__presence"
    :aria-label="t('presence.count', { n: peers.length })"
  >
    <li v-for="peer in peers" :key="peer.clientId">
      <UButton
        size="sm"
        variant="outline"
        color="neutral"
        :aria-label="t('presence.jump', { name: peer.name })"
        :disabled="!peer.blockId"
        @click="peer.blockId && emit('jump', peer.blockId)"
      >
        {{ peer.self ? t("presence.selfOtherTab") : peer.name }}
      </UButton>
    </li>
  </ul>
</template>
