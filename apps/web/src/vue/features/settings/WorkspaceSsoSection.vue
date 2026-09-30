<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import UInput from "@nuxt/ui/components/Input.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref, useId, watch } from "vue";
import { api, ensureOk, ProblemError } from "@/lib/api";
import type { components } from "@/generated/api";
import ConfirmAction from "./ConfirmAction.vue";
import { copyText } from "./clipboard";
import { fieldIssue, parseForm } from "./form";
import { displayedRedirectUri } from "./sso-uri";
import { workspaceOidcForm } from "./workspace-oidc";
import "@/features/settings/settings-shell.css";

type WorkspaceOidcInput = components["schemas"]["WorkspaceOidcBody"];
type RedirectCopyStatus = "copied" | "failed" | null;

const props = defineProps<{ workspaceId: string }>();
const queryClient = useQueryClient();
const formId = useId();
const actionError = ref<string | null>(null);
const copyStatus = ref<RedirectCopyStatus>(null);
const issuer = ref("");
const clientId = ref("");
const clientSecret = ref("");
const label = ref("");
const fieldErrors = ref<Partial<Record<"issuer" | "clientId" | "clientSecret" | "label", string>>>(
  {},
);

const queryKey = ["workspaces", props.workspaceId, "oidc"] as const;
const oidc = useQuery(() => ({
  queryKey,
  queryFn: async () =>
    ensureOk(
      await api.GET("/api/v1/workspaces/{workspace_id}/oidc", {
        params: { path: { workspace_id: props.workspaceId } },
      }),
    ),
  retry: false as const,
}));

const redirectUri = computed(() =>
  displayedRedirectUri(oidc.data.value?.redirectUri, window.location.origin, props.workspaceId),
);
const current = computed(() => oidc.data.value ?? null);
const configured = computed(() => Boolean(current.value?.issuer && current.value.clientId));
const eeRequired = computed(
  () => oidc.error.value instanceof ProblemError && oidc.error.value.status === 404,
);

function failMessage(err: unknown): string {
  return err instanceof ProblemError ? err.title : t("error.network");
}

const save = useMutation({
  mutationFn: async (input: WorkspaceOidcInput) =>
    ensureOk(
      await api.PUT("/api/v1/workspaces/{workspace_id}/oidc", {
        params: { path: { workspace_id: props.workspaceId } },
        body: input,
      }),
    ),
  onSuccess: async () => {
    clientSecret.value = "";
    actionError.value = null;
    await queryClient.invalidateQueries({ queryKey });
  },
  onError: (err) => {
    actionError.value = failMessage(err);
  },
});

const remove = useMutation({
  mutationFn: async () =>
    ensureOk(
      await api.DELETE("/api/v1/workspaces/{workspace_id}/oidc", {
        params: { path: { workspace_id: props.workspaceId } },
      }),
    ),
  onSuccess: async () => {
    actionError.value = null;
    await queryClient.invalidateQueries({ queryKey });
  },
  onError: (err) => {
    actionError.value = failMessage(err);
  },
});

const pending = computed(() => save.isPending.value || remove.isPending.value);
const error = computed(
  () =>
    actionError.value ??
    (oidc.error.value && !eeRequired.value ? failMessage(oidc.error.value) : null),
);

function syncForm(): void {
  issuer.value = current.value?.issuer ?? "";
  clientId.value = current.value?.clientId ?? "";
  clientSecret.value = "";
  label.value = current.value?.label ?? "";
  fieldErrors.value = {};
}

watch(
  () => oidc.data.value,
  () => {
    syncForm();
  },
  { immediate: true },
);

function onCopy(): void {
  void copyText(redirectUri.value).then(
    () => {
      copyStatus.value = "copied";
    },
    () => {
      copyStatus.value = "failed";
    },
  );
}

function onSave(): void {
  fieldErrors.value = {};
  actionError.value = null;
  const values = {
    issuer: issuer.value,
    clientId: clientId.value,
    clientSecret: clientSecret.value,
    label: label.value,
  };
  const parsed = parseForm(workspaceOidcForm, values);
  if (!parsed.ok) {
    fieldErrors.value = {
      issuer: fieldIssue(workspaceOidcForm, values, "issuer") ?? undefined,
      clientId: fieldIssue(workspaceOidcForm, values, "clientId") ?? undefined,
      clientSecret: fieldIssue(workspaceOidcForm, values, "clientSecret") ?? undefined,
      label: fieldIssue(workspaceOidcForm, values, "label") ?? undefined,
    };
    return;
  }
  save.mutate({
    issuer: parsed.data.issuer,
    clientId: parsed.data.clientId,
    clientSecret: parsed.data.clientSecret,
    label: parsed.data.label === "" ? null : parsed.data.label,
  });
}
</script>

<template>
  <details class="settings-disclosure">
    <summary class="settings-disclosure__summary">{{ t("auth.sso.title") }}</summary>
    <div class="settings-disclosure__body flex flex-col gap-4">
      <template v-if="!oidc.isLoading.value && !oidc.isError.value">
        <div class="flex flex-col gap-1.5" data-testid="workspace-sso-redirect-uri">
          <label :for="`${formId}-redirect-uri`" class="font-medium">{{
            t("auth.sso.redirectUri")
          }}</label>
          <p :id="`${formId}-redirect-uri-help`" class="text-muted">{{
            t("auth.sso.redirectUri.help")
          }}</p>
          <div class="flex flex-wrap items-center gap-2">
            <UInput
              :id="`${formId}-redirect-uri`"
              :model-value="redirectUri"
              readonly
              class="min-w-0 flex-1 font-mono"
              autocomplete="off"
              :spellcheck="false"
              :aria-describedby="`${formId}-redirect-uri-help`"
              @focus="($event.target as HTMLInputElement).select()"
            />
            <UButton type="button" size="sm" variant="outline" color="neutral" @click="onCopy">
              {{
                t(
                  copyStatus === "copied"
                    ? "auth.sso.redirectUri.copied"
                    : "auth.sso.redirectUri.copy",
                )
              }}
            </UButton>
          </div>
          <p
            v-if="copyStatus === 'failed'"
            role="alert"
            class="settings-notice settings-notice--danger"
          >
            {{ t("auth.sso.redirectUri.copyFailed") }}
          </p>
        </div>
        <form class="flex flex-col gap-2" novalidate @submit.prevent="onSave">
          <div class="flex flex-col gap-1.5">
            <label :for="`${formId}-issuer`">{{ t("auth.sso.issuer") }}</label>
            <UInput
              :id="`${formId}-issuer`"
              v-model="issuer"
              type="url"
              autocomplete="off"
              :disabled="pending"
              :aria-invalid="fieldErrors.issuer ? true : undefined"
            />
            <p v-if="fieldErrors.issuer" class="text-error" role="alert">{{
              fieldErrors.issuer
            }}</p>
          </div>
          <div class="flex flex-col gap-1.5">
            <label :for="`${formId}-clientId`">{{ t("auth.sso.clientId") }}</label>
            <UInput
              :id="`${formId}-clientId`"
              v-model="clientId"
              autocomplete="off"
              :disabled="pending"
              :aria-invalid="fieldErrors.clientId ? true : undefined"
            />
            <p v-if="fieldErrors.clientId" class="text-error" role="alert">{{
              fieldErrors.clientId
            }}</p>
          </div>
          <div class="flex flex-col gap-1.5">
            <label :for="`${formId}-clientSecret`">{{ t("auth.sso.clientSecret") }}</label>
            <UInput
              :id="`${formId}-clientSecret`"
              v-model="clientSecret"
              type="password"
              autocomplete="off"
              :disabled="pending"
              :aria-invalid="fieldErrors.clientSecret ? true : undefined"
            />
            <p v-if="fieldErrors.clientSecret" class="text-error" role="alert">{{
              fieldErrors.clientSecret
            }}</p>
          </div>
          <div class="flex flex-col gap-1.5">
            <label :for="`${formId}-label`">{{ t("auth.sso.label") }}</label>
            <UInput
              :id="`${formId}-label`"
              v-model="label"
              autocomplete="off"
              :disabled="pending"
              :aria-invalid="fieldErrors.label ? true : undefined"
            />
            <p v-if="fieldErrors.label" class="text-error" role="alert">{{ fieldErrors.label }}</p>
          </div>
          <div class="flex flex-wrap gap-2">
            <UButton type="submit" size="sm" :disabled="pending">{{ t("auth.sso.save") }}</UButton>
            <ConfirmAction
              v-if="configured"
              :title="t('auth.sso.remove.confirm.title')"
              :description="t('auth.sso.remove.confirm.body')"
              :action-label="t('auth.sso.remove')"
              :disabled="pending"
              :run="
                async () => {
                  await remove.mutateAsync().catch(() => undefined);
                }
              "
            >
              {{ t("auth.sso.remove") }}
            </ConfirmAction>
          </div>
        </form>
      </template>
      <p v-if="eeRequired" class="text-muted">{{ t("ee.required") }}</p>
      <p v-if="error" class="text-error" role="alert">{{ error }}</p>
      <p v-if="oidc.isLoading.value" role="status" class="text-muted">{{ t("load.loading") }}</p>
    </div>
  </details>
</template>
