<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { computed } from "vue";
import { REACTIONS } from "@/features/comments/comment-api";
import type { CommentNode } from "@/features/comments/comment-tree";
import CommentCompose from "./CommentCompose.vue";
import type { CommentActions } from "./useCommentActions";

// One comment with reactions, resolve, reply, edit and delete, and its
// replies (features/comments/comment-actions.tsx CommentItem).
const props = defineProps<{
  node: CommentNode;
  actions: CommentActions;
  currentUserId: string;
  readOnly: boolean;
  depth: number;
}>();
const comment = computed(() => props.node.comment);
const isAuthor = computed(() => comment.value.createdBy === props.currentUserId);
const isRoot = computed(() => comment.value.parentId == null);
const resolved = computed(() => comment.value.resolvedAt != null);
const editing = computed(() => props.actions.editingId.value === comment.value.id);
const pending = computed(() => props.actions.pending.value);
const editDraft = computed({
  get: () => props.actions.editDraft.value,
  set: (value: string) => {
    props.actions.setEditDraft(value);
  },
});
const replyDraft = computed({
  get: () => props.actions.replyDraft.value,
  set: (value: string) => {
    props.actions.setReplyDraft(value);
  },
});

function toggleReply(): void {
  props.actions.toggleReply(comment.value.id);
}
function startEdit(): void {
  props.actions.startEdit(comment.value.id, comment.value.body);
}
function cancelEdit(): void {
  props.actions.cancelEdit();
}
function toggleResolved(): void {
  if (resolved.value) props.actions.unresolve.mutate(comment.value.id);
  else props.actions.resolve.mutate(comment.value.id);
}
</script>

<template>
  <li class="comment-thread__item" :style="depth ? { marginLeft: `${depth * 16}px` } : undefined">
    <slot name="before" />
    <p class="comment-thread__body">{{ comment.body }}</p>
    <slot name="meta" />
    <div class="comment-thread__actions">
      <UButton
        v-for="emoji in REACTIONS"
        :key="emoji"
        size="sm"
        :variant="comment.reactions?.[emoji]?.reactedByMe ? 'solid' : 'outline'"
        :color="comment.reactions?.[emoji]?.reactedByMe ? 'primary' : 'neutral'"
        :disabled="pending || readOnly"
        :aria-pressed="comment.reactions?.[emoji]?.reactedByMe ?? false"
        :aria-label="`${t('comment.reaction')} ${emoji}`"
        @click="
          actions.react.mutate({
            id: comment.id,
            emoji,
            on: !(comment.reactions?.[emoji]?.reactedByMe ?? false),
          })
        "
      >
        {{ emoji
        }}{{
          (comment.reactions?.[emoji]?.count ?? 0) > 0
            ? ` ${comment.reactions?.[emoji]?.count}`
            : ""
        }}
      </UButton>
      <UButton
        v-if="!readOnly && isRoot"
        size="sm"
        variant="outline"
        color="neutral"
        :disabled="pending"
        @click="toggleResolved"
      >
        {{ resolved ? t("comment.unresolve") : t("comment.resolve") }}
      </UButton>
      <UButton
        v-if="!readOnly"
        size="sm"
        variant="outline"
        color="neutral"
        :disabled="pending"
        @click="toggleReply"
      >
        {{ t("comment.reply") }}
      </UButton>
      <template v-if="!readOnly && isAuthor">
        <UButton size="sm" variant="outline" color="neutral" :disabled="pending" @click="startEdit">
          {{ t("comment.edit") }}
        </UButton>
        <UButton
          size="sm"
          variant="outline"
          color="neutral"
          :disabled="pending"
          @click="actions.remove.mutate(comment.id)"
        >
          {{ t("comment.delete") }}
        </UButton>
      </template>
    </div>
    <form
      v-if="editing"
      class="comment-thread__compose"
      @submit.prevent="
        actions.patch.mutate({ id: comment.id, body: actions.editDraft.value.trim() })
      "
    >
      <textarea
        v-model="editDraft"
        class="comment-thread__input"
        :aria-label="t('comment.placeholder')"
        :disabled="pending"
      />
      <UButton type="submit" size="sm" :disabled="pending || actions.editDraft.value.trim() === ''">
        {{ t("comment.save") }}
      </UButton>
      <UButton size="sm" variant="outline" color="neutral" @click="cancelEdit">{{
        t("comment.edit.cancel")
      }}</UButton>
    </form>
    <CommentCompose
      v-if="actions.replyToId.value === comment.id && !readOnly"
      v-model:draft="replyDraft"
      :actions="actions"
      reply
      @submit="actions.create.mutate({ text: $event, parentId: comment.id })"
    />
    <ul v-if="node.children.length > 0" class="comment-thread__list">
      <CommentItem
        v-for="child in node.children"
        :key="child.comment.id"
        :node="child"
        :actions="actions"
        :current-user-id="currentUserId"
        :read-only="readOnly"
        :depth="depth + 1"
      />
    </ul>
  </li>
</template>
