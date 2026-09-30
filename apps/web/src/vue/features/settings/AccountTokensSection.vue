<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import UFormField from "@nuxt/ui/components/FormField.vue";
import UInput from "@nuxt/ui/components/Input.vue";
import UPageCard from "@nuxt/ui/components/PageCard.vue";
import USelect from "@nuxt/ui/components/Select.vue";
import { useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref } from "vue";
import { z } from "zod";
import { api, ensureOk, loadErrorMessage, problemMessage } from "@/lib/api";
import type { components } from "@/generated/api";
import { workspacesQuery } from "@/lib/queries";
import { apiTokenCreateInput, apiTokenScope } from "@/lib/validators";
import ConfirmAction from "../../components/ConfirmAction.vue";
import QueryError from "../../components/QueryError.vue";
import QueryLoading from "../../components/QueryLoading.vue";
import RevealedSecret from "./RevealedSecret.vue";
import { parseForm } from "./form";
import { formatExpiry, TOKEN_SCOPE_LABEL, tokenScopeLabels, type TokenScope } from "./token-display";

type ApiToken = components["schemas"]["ApiTokenOutput"];
const client = useQueryClient();
const tokenKey = ["me", "api-tokens"] as const;
const tokens = useQuery({ queryKey: tokenKey, queryFn: async () => ensureOk(await api.GET("/api/v1/me/api-tokens")), retry: false });
const workspaces = useQuery(workspacesQuery);
const workspaceOptions = computed(() => (workspaces.data.value?.items ?? []).map((workspace) => ({ label: workspace.name, value: workspace.id })));
const workspaceNames = computed(() => new Map((workspaces.data.value?.items ?? []).map((workspace) => [workspace.id, workspace.name])));
const name = ref("");
const workspaceId = ref("");
const scopes = ref<TokenScope[]>([]);
const pending = ref(false);
const error = ref<string | null>(null);
// The create answer contains a secret. Keep it out of query/mutation caches.
const revealed = ref<{ id: string; token: string } | null>(null);
const schema = apiTokenCreateInput.pick({ name: true, scopes: true }).extend({ workspaceId: z.string().uuid("i18n:form.invalid") });

async function reloadTokens(): Promise<void> {
  await Promise.all([
    client.invalidateQueries({ queryKey: tokenKey }),
    client.invalidateQueries({ queryKey: ["workspaces"] }),
  ]);
}

async function create(): Promise<void> {
  if (pending.value) return;
  error.value = null;
  const parsed = parseForm(schema, { name: name.value, workspaceId: workspaceId.value, scopes: scopes.value });
  if (!parsed.ok) { error.value = parsed.message; return; }
  pending.value = true;
  revealed.value = null;
  try {
    const created = await ensureOk(await api.POST("/api/v1/me/api-tokens", { body: parsed.data }));
    revealed.value = { id: created.id, token: created.token };
    name.value = "";
    scopes.value = [];
    await reloadTokens();
  } catch (err) {
    error.value = problemMessage(err, "settings.save.failed");
  } finally {
    pending.value = false;
  }
}

async function revoke(token: ApiToken): Promise<void> {
  if (pending.value) return;
  pending.value = true;
  error.value = null;
  try {
    await ensureOk(await api.DELETE("/api/v1/me/api-tokens/{id}", { params: { path: { id: token.id } } }));
    if (revealed.value?.id === token.id) revealed.value = null;
    await reloadTokens();
  } catch (err) {
    error.value = problemMessage(err, "settings.save.failed");
  } finally {
    pending.value = false;
  }
}
</script>

<template>
  <UPageCard as="section" variant="subtle" aria-labelledby="account-tokens-title">
    <h2 id="account-tokens-title" class="settings-section__title">{{ t("settings.tokens") }}</h2>
    <QueryLoading v-if="tokens.isLoading.value || workspaces.isLoading.value" />
    <QueryError v-if="tokens.isError.value || workspaces.isError.value"
      :message="loadErrorMessage(tokens.error.value ?? workspaces.error.value)"
      @retry="() => { void tokens.refetch(); void workspaces.refetch(); }" />
    <form v-if="workspaces.data.value" class="flex flex-col gap-4" novalidate @submit.prevent="create">
      <fieldset class="flex flex-col gap-4" :disabled="pending">
        <legend class="sr-only">{{ t("token.add") }}</legend>
        <UFormField name="tokenWorkspace" :label="t('workspace.name')">
          <USelect id="account-token-workspace" class="w-full" v-model="workspaceId" :items="workspaceOptions" />
        </UFormField>
        <UFormField name="tokenName" :label="t('token.name')">
          <UInput id="account-token-name" class="w-full" v-model="name" autocomplete="off" />
        </UFormField>
        <fieldset class="grid gap-2 sm:grid-cols-2">
          <legend class="mb-2 font-medium">{{ t("token.scopes") }}</legend>
          <label v-for="scope in apiTokenScope.options" :key="scope" class="flex min-h-11 items-center gap-2">
            <input v-model="scopes" type="checkbox" :value="scope" />
            <span>{{ t(TOKEN_SCOPE_LABEL[scope]) }}</span>
          </label>
        </fieldset>
      </fieldset>
      <UButton type="submit" class="w-fit" :disabled="pending || workspaceOptions.length === 0">{{ t("token.create") }}</UButton>
    </form>
    <p v-if="error" role="alert" class="settings-notice settings-notice--danger">{{ error }}</p>
    <div v-if="revealed" data-testid="account-token-secret" class="flex flex-col gap-2">
      <RevealedSecret :key="revealed.id" :value="revealed.token" :label="t('token.once')" :status-text="t('token.once')" />
      <UButton class="w-fit" variant="outline" color="neutral" @click="revealed = null">{{ t("common.dismiss") }}</UButton>
    </div>
    <p v-if="!tokens.isLoading.value && !tokens.isError.value && tokens.data.value?.items.length === 0" class="text-muted">{{ t("token.empty") }}</p>
    <ul class="divide-y divide-default">
      <li v-for="token in tokens.data.value?.items ?? []" :key="token.id" class="flex flex-wrap items-center justify-between gap-3 py-3" data-testid="account-token-row">
        <div class="min-w-0">
          <p class="font-medium break-all">{{ token.name }}</p>
          <p class="text-sm text-muted">{{ workspaceNames.get(token.workspaceId) ?? token.workspaceId }}</p>
          <p class="text-sm text-muted">{{ tokenScopeLabels(token.scopes) }} · {{ formatExpiry(token.expiresAt) }}</p>
        </div>
        <ConfirmAction :title="t('token.revoke.confirm.title')" :description="t('token.revoke.confirm.body', { name: token.name })"
          :action-label="t('token.revoke')" :disabled="pending" :on-confirm="() => revoke(token)">
          {{ t("token.revoke") }}
        </ConfirmAction>
      </li>
    </ul>
  </UPageCard>
</template>
