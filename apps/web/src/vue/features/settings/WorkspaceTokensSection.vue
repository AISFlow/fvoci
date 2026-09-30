<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import UInput from "@nuxt/ui/components/Input.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref, useId } from "vue";
import { api, ensureOk, ProblemError } from "@/lib/api";
import type { ApiTokenCreateInput, ApiTokenOutput } from "@/lib/contracts";
import { workspaceApiTokensQuery } from "@/lib/queries";
import { apiTokenCreateInput, apiTokenScope } from "@/lib/validators";
import ConfirmDialog from "./ConfirmDialog.vue";
import RevealedSecret from "./RevealedSecret.vue";
import { parseForm } from "./form";
import { formatExpiry, scopeDomId, TOKEN_SCOPE_LABEL, tokenScopeLabels, type TokenScope } from "./token-display";
import { toggleItem } from "./toggle";
import "@/features/settings/settings-shell.css";

const props = defineProps<{ workspaceId: string }>();
const queryClient = useQueryClient();
const formId = useId();
const tokens = useQuery(() => workspaceApiTokensQuery(props.workspaceId));
const name = ref("");
const scopes = ref<TokenScope[]>([]);
const unlimited = ref(false);
const service = ref(false);
const nameError = ref<string | null>(null);
const scopesError = ref<string | null>(null);
const formError = ref<string | null>(null);
const revealed = ref<{ id: string; token: string } | null>(null);
const revokeTarget = ref<ApiTokenOutput | null>(null);
const revokeError = ref<string | null>(null);

const items = computed(() => tokens.data.value?.items ?? []);
const listError = computed(() =>
  tokens.error.value instanceof ProblemError
    ? tokens.error.value.title
    : tokens.error.value
      ? t("error.network")
      : null,
);

const create = useMutation({
  mutationFn: async (input: ApiTokenCreateInput) =>
    ensureOk(
      await api.POST("/api/v1/workspaces/{workspace_id}/api-tokens", {
        params: { path: { workspace_id: props.workspaceId } },
        body: input,
      }),
    ),
  onSuccess: async (created) => {
    revealed.value = { id: created.id, token: created.token };
    await queryClient.invalidateQueries({ queryKey: ["workspaces", props.workspaceId, "api-tokens"] });
  },
});

const revoke = useMutation({
  mutationFn: async (id: string) =>
    ensureOk(
      await api.DELETE("/api/v1/workspaces/{workspace_id}/api-tokens/{id}", {
        params: { path: { workspace_id: props.workspaceId, id } },
      }),
    ),
  onSuccess: async () => {
    await queryClient.invalidateQueries({ queryKey: ["workspaces", props.workspaceId, "api-tokens"] });
  },
});

function onScope(scope: TokenScope, event: Event): void {
  scopes.value = toggleItem(scopes.value, scope, (event.target as HTMLInputElement).checked);
}

function onCreate(): void {
  nameError.value = null;
  scopesError.value = null;
  formError.value = null;
  const parsed = parseForm(apiTokenCreateInput, {
    name: name.value,
    scopes: scopes.value,
    unlimited: unlimited.value,
    service: service.value,
  });
  if (!parsed.ok) {
    if (scopes.value.length === 0) scopesError.value = t("token.scopes.required");
    else nameError.value = parsed.message;
    return;
  }
  void create.mutateAsync(parsed.data).then(
    () => {
      name.value = "";
      scopes.value = [];
      unlimited.value = false;
      service.value = false;
    },
    (err: unknown) => {
      formError.value = err instanceof ProblemError ? err.title : t("error.network");
    },
  );
}

function confirmRevoke(): void {
  const target = revokeTarget.value;
  if (!target) return;
  void revoke.mutateAsync(target.id).then(
    () => {
      if (revealed.value?.id === target.id) revealed.value = null;
      revokeTarget.value = null;
    },
    (err: unknown) => {
      revokeError.value = err instanceof ProblemError ? err.title : t("error.network");
    },
  );
}
</script>

<template>
  <details class="settings-disclosure">
    <summary class="settings-disclosure__summary">{{ t("settings.tokens") }}</summary>
    <div class="settings-disclosure__body">
      <form class="flex flex-col gap-2" novalidate @submit.prevent="onCreate">
        <p class="font-medium">{{ t("token.add") }}</p>
        <div class="flex flex-wrap items-end gap-2">
          <div class="flex min-w-40 flex-1 flex-col gap-1.5">
            <label :for="`${formId}-name`">{{ t("token.name") }}</label>
            <UInput
              :id="`${formId}-name`"
              v-model="name"
              :disabled="create.isPending.value"
              :aria-invalid="nameError ? true : undefined"
              :aria-describedby="nameError ? `${formId}-name-error` : undefined"
            />
            <p v-if="nameError" :id="`${formId}-name-error`" class="text-error" role="alert">{{ nameError }}</p>
          </div>
          <UButton type="submit" size="sm" :disabled="create.isPending.value">{{ t("token.create") }}</UButton>
        </div>
        <fieldset class="flex flex-col gap-1.5">
          <legend class="font-medium">{{ t("token.scopes") }}</legend>
          <div v-for="scope in apiTokenScope.options" :key="scope" class="flex min-h-11 items-center gap-2">
            <input
              :id="`${formId}-${scopeDomId(scope)}`"
              type="checkbox"
              class="size-4"
              :checked="scopes.includes(scope)"
              :disabled="create.isPending.value"
              @change="onScope(scope, $event)"
            />
            <label :for="`${formId}-${scopeDomId(scope)}`">{{ t(TOKEN_SCOPE_LABEL[scope]) }}</label>
          </div>
          <p v-if="scopesError" class="text-error" role="alert">{{ scopesError }}</p>
        </fieldset>
        <div class="flex flex-wrap gap-4">
          <label class="flex min-h-11 items-center gap-2">
            <input v-model="unlimited" type="checkbox" class="size-4" :disabled="create.isPending.value" />
            <span>{{ t("token.unlimited") }}</span>
          </label>
          <label class="flex min-h-11 items-center gap-2">
            <input v-model="service" type="checkbox" class="size-4" :disabled="create.isPending.value" />
            <span>{{ t("token.service") }}</span>
          </label>
        </div>
        <p v-if="formError" role="alert" class="settings-notice settings-notice--danger">{{ formError }}</p>
      </form>
      <RevealedSecret
        v-if="revealed"
        :key="revealed.id"
        :value="revealed.token"
        :status-text="t('token.once')"
        :label="t('token.once')"
      />
      <p v-if="tokens.isPending.value" role="status">{{ t("load.loading") }}</p>
      <p v-if="listError" role="alert" class="settings-notice settings-notice--danger">{{ listError }}</p>
      <p v-if="!tokens.isPending.value && !listError && items.length === 0" class="text-muted">{{ t("token.empty") }}</p>
      <ul v-if="items.length > 0" class="flex flex-col divide-y">
        <li
          v-for="row in items"
          :key="row.id"
          class="flex min-w-0 flex-col gap-2 py-3 first:pt-0 last:pb-0 sm:flex-row sm:items-center sm:justify-between"
        >
          <div class="min-w-0">
            <p class="font-medium break-keep">
              {{ row.name }}
              <span v-if="row.userId === null" class="ml-2 text-muted">{{ t("token.service") }}</span>
            </p>
            <p class="text-muted break-keep">{{ tokenScopeLabels(row.scopes) }}</p>
            <p class="text-muted">{{ t("token.expires") }}: {{ formatExpiry(row.expiresAt ?? null) }}</p>
          </div>
          <UButton
            type="button"
            size="sm"
            variant="outline"
            color="neutral"
            :disabled="revoke.isPending.value"
            @click="revokeTarget = row"
          >
            {{ t("token.revoke") }}
          </UButton>
        </li>
      </ul>
    </div>
    <ConfirmDialog
      :open="revokeTarget !== null"
      :title="t('token.revoke.confirm.title')"
      :body="revokeTarget ? t('token.revoke.confirm.body', { name: revokeTarget.name }) : ''"
      :action-label="t('token.revoke')"
      :pending="revoke.isPending.value"
      :error="revokeError"
      @close="
        revokeTarget = null;
        revokeError = null;
      "
      @confirm="confirmRevoke"
    />
  </details>
</template>
