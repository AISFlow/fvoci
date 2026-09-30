<script setup lang="ts">
import { isI18nKey, t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref, useId } from "vue";
import {
  copyText,
  createDocumentShareLink,
  formatShareDate,
  revokeShareLink,
} from "@/features/share/share-api";
import { problemMessage } from "@/lib/api";
import { publicInstanceQuery, refreshPublicInstance } from "@/lib/queries/instance";
import { documentShareLinksQuery, type ShareDocumentTarget } from "@/lib/queries/share";
import { SHARE_POLICY_DEFAULT, selectedShareExpires, shareExpiresOptions } from "@/lib/share-links";
import NativeModal from "../../components/NativeModal.vue";
import "@/features/projects/projects.css";
import "@/features/share/share.css";

// The document's share links (features/share/share-dialog.tsx). Expiry
// choices and whether links can be created come from the public instance
// share policy; the server enforces the same policy on create.
const props = defineProps<{ workspaceId: string; target: ShareDocumentTarget }>();
const queryClient = useQueryClient();
const titleId = useId();
const expiresId = useId();
const open = ref(false);
const createdUrl = ref<string | null>(null);
const copied = ref(false);
const actionError = ref<string | null>(null);
const expiresInDays = ref<number | null>(null);
const instance = useQuery(publicInstanceQuery);
const policy = computed(() => instance.data.value?.values.share ?? SHARE_POLICY_DEFAULT);
const expiresOptions = computed(() => shareExpiresOptions(policy.value));
const selectedExpires = computed(() => selectedShareExpires(expiresInDays.value, policy.value));
const createEnabled = computed(() => instance.isSuccess.value && policy.value.enabled);
const linksOptions = computed(() => documentShareLinksQuery(props.workspaceId, props.target));
const list = useQuery(() => ({ ...linksOptions.value, enabled: open.value }));

async function invalidateLinks(): Promise<void> {
  await queryClient.invalidateQueries({ queryKey: linksOptions.value.queryKey });
}

const create = useMutation({
  mutationFn: (days: number) => createDocumentShareLink(props.workspaceId, props.target, days),
  onSuccess: async (created) => {
    actionError.value = null;
    copied.value = false;
    createdUrl.value = created.url;
    await invalidateLinks();
  },
  onError: (err: unknown) => {
    actionError.value = problemMessage(err, "error.share.failed");
  },
});

const revoke = useMutation({
  mutationFn: (id: string) => revokeShareLink(props.workspaceId, id),
  onSuccess: async () => {
    actionError.value = null;
    await invalidateLinks();
  },
  onError: (err: unknown) => {
    actionError.value = problemMessage(err, "error.share.failed");
  },
});

const pending = computed(() => create.isPending.value || revoke.isPending.value);
const links = computed(() => list.data.value?.items ?? []);
const error = computed(
  () =>
    actionError.value ??
    (instance.error.value
      ? problemMessage(instance.error.value, "error.share.failed")
      : !policy.value.enabled
        ? t("share.disabled")
        : null) ??
    (list.error.value ? problemMessage(list.error.value, "error.share.failed") : null),
);

function expiresLabel(days: number): string {
  const key = `share.expires.${days}`;
  return isI18nKey(key) ? t(key) : String(days);
}

// `/instance` is cached (query 30 s, HTTP 60 s); an admin may just have
// changed the policy. The server enforces it on create either way.
function openDialog(): void {
  void refreshPublicInstance(queryClient).catch(() => undefined);
  open.value = true;
}

function close(): void {
  open.value = false;
  createdUrl.value = null;
  actionError.value = null;
}

function copy(url: string): void {
  void copyText(url).then(
    () => {
      copied.value = true;
    },
    () => {
      copied.value = false;
    },
  );
}

function revokeRow(id: string): void {
  if (!window.confirm(`${t("share.revoke.confirm.title")}\n${t("share.revoke.confirm.body")}`)) return;
  revoke.mutate(id);
}
</script>

<template>
  <UButton size="sm" variant="outline" color="neutral" @click="openDialog">{{ t("share.create") }}</UButton>
  <NativeModal :open="open" :labelled-by="titleId" @close="close">
    <div class="share-dialog">
      <div>
        <h2 :id="titleId" class="project-dialog__title">{{ t("share.create") }}</h2>
        <p class="share-dialog__empty">{{ t("share.document") }}</p>
      </div>
      <section class="share-dialog__stack">
        <div class="share-dialog__field">
          <label :for="expiresId" class="text-sm font-medium">{{ t("share.expiresIn") }}</label>
          <select
            :id="expiresId"
            class="document-page__field-select"
            :value="String(selectedExpires)"
            :disabled="!createEnabled || pending"
            @change="expiresInDays = Number(($event.target as HTMLSelectElement).value)"
          >
            <option v-for="days in expiresOptions" :key="days" :value="String(days)">{{ expiresLabel(days) }}</option>
          </select>
        </div>
        <UButton size="sm" class="w-fit" :disabled="pending || !createEnabled" @click="create.mutate(selectedExpires)">
          {{ t("share.create") }}
        </UButton>
        <!-- WHY: 공유 링크 원문은 생성 직후 한 번만 보여 준다. 목록은 id·만료만 둔다. -->
        <div v-if="createdUrl" class="share-dialog__secret">
          <input
            :aria-label="t('share.url')"
            readonly
            :value="createdUrl"
            class="h-9 w-full rounded-md border border-default bg-default px-3 font-mono text-sm"
          />
          <UButton size="sm" variant="outline" color="neutral" @click="copy(createdUrl)">
            {{ copied ? t("share.copied") : t("share.copy") }}
          </UButton>
        </div>
        <p v-if="error" class="share-dialog__alert" role="alert">{{ error }}</p>
      </section>
      <section class="share-dialog__stack">
        <p v-if="list.isLoading.value" role="status" class="share-dialog__empty">{{ t("load.loading") }}</p>
        <p v-if="!list.isLoading.value && !list.isError.value && links.length === 0" class="share-dialog__empty">
          {{ t("share.empty") }}
        </p>
        <table v-if="links.length > 0" class="share-dialog__table">
          <thead>
            <tr>
              <th scope="col">{{ t("share.document") }}</th>
              <th scope="col">{{ t("share.expires") }}</th>
              <th scope="col"><span class="sr-only">{{ t("share.revoke") }}</span></th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="row in links" :key="row.id">
              <td>{{ row.documentId ? t("share.document") : t("share.project") }}</td>
              <td>{{ formatShareDate(row.expiresAt) }}</td>
              <td class="text-right">
                <UButton size="sm" variant="outline" color="error" :disabled="pending" @click="revokeRow(row.id)">
                  {{ t("share.revoke") }}
                </UButton>
              </td>
            </tr>
          </tbody>
        </table>
      </section>
      <div class="flex justify-end">
        <UButton size="sm" variant="outline" color="neutral" @click="close">{{ t("common.dismiss") }}</UButton>
      </div>
    </div>
  </NativeModal>
</template>
