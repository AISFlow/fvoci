<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import UInput from "@nuxt/ui/components/Input.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref, useId } from "vue";
import {
  WEBHOOK_EVENTS,
  webhookCreateProblemKey,
  webhookEventLabel,
} from "@/features/settings/webhook-events";
import { api, ensureOk, ProblemError } from "@/lib/api";
import type { WebhookCreateInput, WebhookCreatedOutput, WebhookOutput } from "@/lib/contracts";
import { workspaceWebhooksQuery } from "@/lib/queries";
import { webhookCreateInput } from "@/lib/validators";
import ConfirmDialog from "./ConfirmDialog.vue";
import RevealedSecret from "./RevealedSecret.vue";
import { parseForm } from "./form";
import { toggleItem } from "./toggle";
import "@/features/settings/settings-shell.css";

class WebhookCreateError extends Error {}

const props = defineProps<{ workspaceId: string }>();
const queryClient = useQueryClient();
const formId = useId();
const webhooks = useQuery(() => workspaceWebhooksQuery(props.workspaceId));
const url = ref("");
const events = ref<string[]>([]);
const urlError = ref<string | null>(null);
const eventsError = ref<string | null>(null);
const formError = ref<string | null>(null);
const revealed = ref<{ id: string; secret: string } | null>(null);
const deleteTarget = ref<WebhookOutput | null>(null);
const deleteError = ref<string | null>(null);

const queryKey = computed(() => workspaceWebhooksQuery(props.workspaceId).queryKey);
const items = computed(() => webhooks.data.value?.items ?? []);

function failMessage(err: unknown): string {
  if (err instanceof WebhookCreateError) return err.message;
  return err instanceof ProblemError ? err.title : t("error.network");
}

const listError = computed(() => (webhooks.error.value ? failMessage(webhooks.error.value) : null));

const create = useMutation({
  mutationFn: async (input: WebhookCreateInput): Promise<WebhookCreatedOutput> => {
    const result = await api.POST("/api/v1/workspaces/{workspace_id}/webhooks", {
      params: { path: { workspace_id: props.workspaceId } },
      body: input,
    });
    if (result.error) {
      const key = webhookCreateProblemKey(result.error.code, result.error.source);
      if (key) throw new WebhookCreateError(t(key));
    }
    return ensureOk(result);
  },
  onSuccess: async (created) => {
    revealed.value = { id: created.id, secret: created.secret };
    await queryClient.invalidateQueries({ queryKey: queryKey.value });
  },
});

const remove = useMutation({
  mutationFn: async (id: string) =>
    ensureOk(
      await api.DELETE("/api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}", {
        params: { path: { workspace_id: props.workspaceId, webhook_id: id } },
      }),
    ),
  onSuccess: async () => {
    await queryClient.invalidateQueries({ queryKey: queryKey.value });
  },
});

function onCreate(): void {
  urlError.value = null;
  eventsError.value = null;
  formError.value = null;
  const parsed = parseForm(webhookCreateInput, { url: url.value, events: events.value });
  if (!parsed.ok) {
    if (events.value.length === 0) eventsError.value = t("webhook.events.required");
    else urlError.value = parsed.message;
    return;
  }
  void create.mutateAsync({ url: parsed.data.url, events: parsed.data.events }).then(
    () => {
      url.value = "";
      events.value = [];
    },
    (err: unknown) => {
      formError.value = failMessage(err);
    },
  );
}

function confirmDelete(): void {
  const target = deleteTarget.value;
  if (!target) return;
  void remove.mutateAsync(target.id).then(
    () => {
      if (revealed.value?.id === target.id) revealed.value = null;
      deleteTarget.value = null;
    },
    (err: unknown) => {
      deleteError.value = failMessage(err);
    },
  );
}
</script>

<template>
  <details class="settings-disclosure">
    <summary class="settings-disclosure__summary">{{ t("settings.webhooks") }}</summary>
    <div class="settings-disclosure__body">
      <form class="flex flex-col gap-2" novalidate @submit.prevent="onCreate">
        <p class="font-medium">{{ t("webhook.add") }}</p>
        <div class="flex flex-col gap-1.5">
          <label :for="`${formId}-url`">{{ t("webhook.url") }}</label>
          <UInput
            :id="`${formId}-url`"
            v-model="url"
            type="url"
            inputmode="url"
            autocomplete="off"
            :maxlength="2048"
            :disabled="create.isPending.value"
            :aria-invalid="urlError ? true : undefined"
            :aria-describedby="urlError ? `${formId}-url-error` : undefined"
          />
          <p v-if="urlError" :id="`${formId}-url-error`" class="text-error" role="alert">{{
            urlError
          }}</p>
        </div>
        <fieldset class="flex flex-col gap-1.5">
          <legend class="font-medium">{{ t("webhook.events") }}</legend>
          <div class="grid gap-x-4 sm:grid-cols-2">
            <label
              v-for="verb in WEBHOOK_EVENTS"
              :key="verb"
              class="flex min-h-11 items-center gap-2"
            >
              <input
                :id="`${formId}-${verb.replaceAll('.', '-')}`"
                type="checkbox"
                class="size-4"
                :checked="events.includes(verb)"
                :disabled="create.isPending.value"
                @change="
                  events = toggleItem(events, verb, ($event.target as HTMLInputElement).checked)
                "
              />
              <span>{{ webhookEventLabel(verb) }}</span>
            </label>
          </div>
          <p v-if="eventsError" class="text-error" role="alert">{{ eventsError }}</p>
        </fieldset>
        <UButton type="submit" size="sm" class="w-fit" :disabled="create.isPending.value">{{
          t("webhook.create")
        }}</UButton>
        <p v-if="formError" role="alert" class="settings-notice settings-notice--danger">{{
          formError
        }}</p>
      </form>
      <RevealedSecret
        v-if="revealed"
        :key="revealed.id"
        :value="revealed.secret"
        :status-text="t('webhook.secret.once')"
        :label="t('webhook.secret.label')"
        :copy-label="t('webhook.copy')"
        :copied-label="t('webhook.copied')"
      />
      <p v-if="webhooks.isPending.value" role="status">{{ t("load.loading") }}</p>
      <p v-if="listError" role="alert" class="settings-notice settings-notice--danger">{{
        listError
      }}</p>
      <p v-if="!webhooks.isPending.value && !listError && items.length === 0" class="text-muted">{{
        t("webhook.empty")
      }}</p>
      <ul
        v-if="items.length > 0"
        class="flex flex-col divide-y"
        :aria-label="t('settings.webhooks')"
      >
        <li
          v-for="row in items"
          :key="row.id"
          class="flex min-w-0 flex-col gap-2 py-3 first:pt-0 last:pb-0 sm:flex-row sm:items-center sm:justify-between"
        >
          <div class="min-w-0">
            <p class="font-mono break-all">{{ row.url }}</p>
            <p class="text-muted break-keep">{{ row.events.map(webhookEventLabel).join(", ") }}</p>
          </div>
          <UButton
            type="button"
            size="sm"
            variant="outline"
            color="neutral"
            :disabled="remove.isPending.value"
            :aria-label="`${t('webhook.delete')} ${row.url}`"
            @click="
              deleteError = null;
              deleteTarget = row;
            "
          >
            {{ t("webhook.delete") }}
          </UButton>
        </li>
      </ul>
    </div>
    <ConfirmDialog
      :open="deleteTarget !== null"
      :title="t('webhook.delete.confirm.title')"
      :body="deleteTarget ? t('webhook.delete.confirm.body', { url: deleteTarget.url }) : ''"
      :action-label="t('webhook.delete')"
      :pending="remove.isPending.value"
      :error="deleteError"
      @close="
        deleteTarget = null;
        deleteError = null;
      "
      @confirm="confirmDelete"
    />
  </details>
</template>
