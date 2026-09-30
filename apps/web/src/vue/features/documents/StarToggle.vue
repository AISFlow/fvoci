<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref } from "vue";
import { toggleStar } from "@/features/share/share-api";
import { problemMessage } from "@/lib/api";
import { starsQuery } from "@/lib/queries/share";

// Star/unstar the page's item (features/share/star-toggle.tsx).
const props = defineProps<{ workspaceId: string; type: "document" | "task"; targetId: string }>();
const queryClient = useQueryClient();
const stars = useQuery(() => starsQuery(props.workspaceId));
const error = ref<string | null>(null);
const star = computed(() =>
  stars.data.value?.items.find((item) => item.type === props.type && item.targetId === props.targetId),
);

const toggle = useMutation({
  mutationFn: () => toggleStar(props.workspaceId, star.value, props.type, props.targetId),
  onSuccess: async () => {
    error.value = null;
    await queryClient.invalidateQueries({ queryKey: ["stars", props.workspaceId] });
  },
  onError: (err: unknown) => {
    error.value = problemMessage(err, "load.failed");
  },
});
</script>

<template>
  <UButton
    size="sm"
    variant="outline"
    color="neutral"
    :disabled="stars.isLoading.value || toggle.isPending.value"
    @click="toggle.mutate()"
  >
    <span aria-hidden="true" class="mr-1">{{ star ? "★" : "☆" }}</span>{{ star ? t("cmdk.unstar") : t("cmdk.star") }}
  </UButton>
  <p v-if="error" role="alert" class="document-page__error">{{ error }}</p>
</template>
