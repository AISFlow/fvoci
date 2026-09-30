<script setup lang="ts">
import { t } from "@fvoci/i18n";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref, useId } from "vue";
import { taskAttachmentsQuery } from "@/features/tasks/queries";
import { createTaskAttachmentBridge } from "@/features/workspace/attachment-upload";
import { api, ensureOk, ProblemError } from "@/lib/api";
import ConfirmActionButton from "../../components/ConfirmActionButton.vue";

function attachmentErrorMessage(error: unknown): string {
  return error instanceof ProblemError ? error.title : t("error.network");
}

const props = defineProps<{
  workspaceId: string;
  taskId: string;
  readOnly: boolean;
}>();

const queryClient = useQueryClient();
const inputId = useId();
const bridge = createTaskAttachmentBridge(props.workspaceId, props.taskId);
const attachments = useQuery(() => taskAttachmentsQuery(props.workspaceId, props.taskId));
const error = ref<string | null>(null);
const uploading = ref(false);
const items = computed(() => attachments.data.value?.items ?? []);
const hidden = computed(() => props.readOnly && items.value.length === 0);

const remove = useMutation({
  mutationFn: async (attachmentId: string) =>
    ensureOk(
      await api.DELETE("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}", {
        params: { path: { workspace_id: props.workspaceId, attachment_id: attachmentId } },
      }),
    ),
  onSuccess: async () => {
    error.value = null;
    await queryClient.invalidateQueries({
      queryKey: taskAttachmentsQuery(props.workspaceId, props.taskId).queryKey,
    });
  },
  onError: (err) => {
    error.value = attachmentErrorMessage(err);
  },
});

async function attach(files: File[]): Promise<void> {
  if (props.readOnly || files.length === 0) return;
  error.value = null;
  uploading.value = true;
  try {
    await Promise.all(files.map((file) => bridge.upload(file, () => undefined)));
  } catch (err) {
    error.value = attachmentErrorMessage(err);
  } finally {
    uploading.value = false;
    await queryClient.invalidateQueries({
      queryKey: taskAttachmentsQuery(props.workspaceId, props.taskId).queryKey,
    });
  }
}

async function onPaste(event: ClipboardEvent): Promise<void> {
  if (props.readOnly) return;
  const files = [...(event.clipboardData?.files ?? [])];
  if (files.length === 0) return;
  event.preventDefault();
  await attach(files);
}

async function onPick(event: Event): Promise<void> {
  const input = event.currentTarget as HTMLInputElement;
  const files = [...(input.files ?? [])];
  input.value = "";
  await attach(files);
}
</script>

<template>
  <section
    v-if="!hidden"
    class="group/attachments flex flex-col gap-1.5"
    :aria-label="t('search.tab.attachment')"
    :tabindex="readOnly ? undefined : 0"
    @paste="onPaste"
  >
    <div v-if="!readOnly" class="flex items-center gap-2">
      <label
        :for="inputId"
        class="inline-flex h-9 cursor-pointer items-center rounded-md border border-default px-3 text-sm"
      >
        {{ t("task.attach.pick") }}
      </label>
      <input
        :id="inputId"
        type="file"
        multiple
        class="sr-only"
        :disabled="uploading"
        @change="onPick"
      />
      <p class="text-sm text-muted">
        {{ uploading ? t("task.attach.uploading") : t("task.attach.paste") }}
      </p>
    </div>
    <ul class="flex flex-col gap-1 text-sm">
      <li v-for="a in items" :key="a.id" class="flex items-center justify-between gap-2">
        <a
          v-if="a.completedAt"
          class="flex min-w-0 items-center gap-2 underline"
          :href="`/api/v1/workspaces/${workspaceId}/attachments/${a.id}/download`"
          :download="a.name"
        >
          <img
            v-if="a.preview"
            :src="`/api/v1/workspaces/${workspaceId}/attachments/${a.id}/download?variant=preview`"
            :width="a.preview.width"
            :height="a.preview.height"
            alt=""
            loading="lazy"
            class="h-10 w-auto max-w-16 shrink-0 rounded-sm object-cover"
          />
          <span class="truncate break-all">{{ a.name }}</span>
        </a>
        <span v-else class="min-w-0 truncate break-all text-muted">{{ a.name }}</span>
        <ConfirmActionButton
          v-if="!readOnly"
          :title="t('task.attach.delete.confirm.title')"
          :description="t('task.attach.delete.confirm.body', { name: a.name })"
          :action-label="t('task.attach.delete')"
          :disabled="remove.isPending.value"
          :action="() => remove.mutateAsync(a.id).then(() => undefined)"
        >
          {{ t("task.attach.delete") }}
        </ConfirmActionButton>
      </li>
    </ul>
    <p v-if="error" role="alert" class="text-sm text-error">{{ error }}</p>
  </section>
</template>
