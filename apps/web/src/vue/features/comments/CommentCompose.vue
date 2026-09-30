<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { appendGroupMention } from "@/features/comments/comment-api";
import type { CommentActions } from "./useCommentActions";

// A root comment or reply form with the group-mention picker.
const props = defineProps<{
  actions: CommentActions;
  draft: string;
  reply?: boolean;
}>();
const emit = defineEmits<{ "update:draft": [value: string]; submit: [text: string] }>();

function submit(): void {
  const text = props.draft.trim();
  if (text) emit("submit", text);
}

function mention(event: Event): void {
  const select = event.target as HTMLSelectElement;
  const groupId = select.value;
  select.value = "";
  const group = props.actions.groups.value.find((item) => item.id === groupId);
  if (group) emit("update:draft", appendGroupMention(props.draft, group.name));
}
</script>

<template>
  <form
    class="comment-thread__compose"
    :data-comment-compose="reply ? undefined : ''"
    :data-comment-reply="reply ? '' : undefined"
    @submit.prevent="submit"
  >
    <textarea
      class="comment-thread__input"
      :value="draft"
      :aria-label="t('comment.placeholder')"
      :placeholder="t('comment.placeholder')"
      :disabled="actions.pending.value"
      @input="emit('update:draft', ($event.target as HTMLTextAreaElement).value)"
    />
    <div class="comment-thread__compose-row">
      <select
        v-if="actions.groups.value.length > 0"
        class="comment-thread__mention"
        :aria-label="t('group.mention')"
        value=""
        :disabled="actions.pending.value"
        @change="mention"
      >
        <option value="">{{ t("group.mention") }}</option>
        <option v-for="group in actions.groups.value" :key="group.id" :value="group.id">{{ group.name }}</option>
      </select>
      <UButton type="submit" size="sm" :disabled="actions.pending.value || draft.trim() === ''">
        {{ t("comment.submit") }}
      </UButton>
    </div>
  </form>
</template>
