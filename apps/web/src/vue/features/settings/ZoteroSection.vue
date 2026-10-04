<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import UInput from "@nuxt/ui/components/Input.vue";
import USelect from "@nuxt/ui/components/Select.vue";
import { useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, onBeforeUnmount, ref, useId, watch } from "vue";
import { ProblemError } from "@/lib/api";
import { documentPath, itemPath } from "@/lib/href";
import { lookupTarget } from "@/features/zotero/api";
import {
  actorKey,
  connect,
  connectorsQuery,
  disconnect,
  ensurePersonalWorkspace,
  libraryQuery,
  link,
  sync,
  workspacesQuery,
  type ActorScope,
  type Reference,
} from "@/features/zotero/api";
import ConfirmDialog from "./ConfirmDialog.vue";
import "@/features/settings/settings-shell.css";

const props = defineProps<{ userId: string; sessionId: string }>();
const client = useQueryClient();
const formId = useId();
const scope = computed(() => ({ userId: props.userId, sessionId: props.sessionId }));
const workspaces = useQuery(() => workspacesQuery(scope.value));
const createdWorkspace = ref("");
const createdSlug = ref("");
const workspace = computed(
  () =>
    createdWorkspace.value ||
    workspaces.data.value?.items.find((item) => item.kind === "personal")?.id ||
    "",
);
const workspaceSlug = computed(
  () =>
    createdSlug.value ||
    workspaces.data.value?.items.find((item) => item.id === workspace.value)?.slug ||
    "",
);
const connectors = useQuery(() => ({
  ...connectorsQuery(scope.value, workspace.value),
  enabled: Boolean(workspace.value),
}));
const selected = ref("");
const library = useQuery(() => ({
  ...libraryQuery(scope.value, workspace.value, selected.value),
  enabled: Boolean(workspace.value && selected.value),
}));
const kind = ref<"user" | "group">("user");
const remoteId = ref("");
const libraryUrl = ref("");
const secret = ref("");
const busy = ref(false);
const error = ref<string | null>(null);
const notice = ref<string | null>(null);
const disconnectOpen = ref(false);
const linkReference = ref<Reference | null>(null);
const targetKind = ref("document");
const targetId = ref("");
let epoch = 0;
function retire(): void {
  epoch++;
  secret.value = "";
  remoteId.value = "";
  libraryUrl.value = "";
  kind.value = "user";
  targetKind.value = "document";
  busy.value = false;
  error.value = null;
  notice.value = null;
  selected.value = "";
  createdWorkspace.value = "";
  createdSlug.value = "";
  linkReference.value = null;
  targetId.value = "";
  disconnectOpen.value = false;
}
watch(() => [props.userId, props.sessionId], retire);
onBeforeUnmount(retire);
watch(
  () => connectors.data.value?.connectors,
  (items) => {
    if (selected.value && !items?.some((item) => item.id === selected.value)) selected.value = "";
    if (!selected.value) selected.value = items?.[0]?.id ?? "";
  },
);
watch(selected, () => {
  linkReference.value = null;
  targetId.value = "";
  notice.value = null;
  error.value = null;
});
const typeOptions = computed(() => [
  { label: t("zotero.userLibrary"), value: "user" },
  { label: t("zotero.groupLibrary"), value: "group" },
]);
const connectorOptions = computed(
  () =>
    connectors.data.value?.connectors.map((item) => ({
      label: `${item.libraryType === "user" ? t("zotero.userLibrary") : t("zotero.groupLibrary")} ${item.remoteLibraryId}`,
      value: item.id,
    })) ?? [],
);
const targetOptions = computed(() => [
  { label: t("zotero.document"), value: "document" },
  { label: t("zotero.task"), value: "task" },
]);
function fail(err: unknown): string {
  if (err instanceof ProblemError) {
    if (err.reason === "zotero_denied") return t("zotero.denied");
    if (err.reason === "zotero_delayed") return t("zotero.delayed");
    if (err.status === 503) return t("zotero.transient");
    return err.title;
  }
  return t("error.network");
}
function sameActor(captured: ActorScope, capturedEpoch: number): boolean {
  return (
    capturedEpoch === epoch &&
    captured.userId === props.userId &&
    captured.sessionId === props.sessionId
  );
}
async function refresh(captured: ActorScope, ws: string, connector: string): Promise<void> {
  await client.invalidateQueries({ queryKey: [...actorKey(captured), ws, "connectors"] });
  if (connector)
    await client.invalidateQueries({ queryKey: libraryQuery(captured, ws, connector).queryKey });
}
async function onConnect(): Promise<void> {
  if (busy.value) return;
  const captured = { ...scope.value };
  const capturedEpoch = epoch;
  const input = {
    libraryType: kind.value,
    remoteLibraryId: remoteId.value,
    libraryUrl: libraryUrl.value,
    apiKey: secret.value,
  };
  secret.value = "";
  error.value = null;
  busy.value = true;
  try {
    const personal = workspace.value ? null : await ensurePersonalWorkspace();
    const ws = workspace.value || personal?.id || "";
    if (!sameActor(captured, capturedEpoch)) return;
    const result = await connect(ws, input);
    if (!sameActor(captured, capturedEpoch)) return;
    createdWorkspace.value = ws;
    createdSlug.value = personal?.slug || workspaceSlug.value;
    selected.value = result.id;
    await refresh(captured, ws, result.id);
  } catch (err) {
    if (sameActor(captured, capturedEpoch)) error.value = fail(err);
  } finally {
    input.apiKey = "";
    if (sameActor(captured, capturedEpoch)) busy.value = false;
  }
}
async function action(operation: "sync" | "disconnect" | "link"): Promise<void> {
  if (busy.value || !workspace.value || !selected.value) return;
  const captured = { ...scope.value };
  const capturedEpoch = epoch;
  const ws = workspace.value;
  const connector = selected.value;
  const reference = linkReference.value;
  const target = targetId.value;
  const targetType = targetKind.value;
  busy.value = true;
  error.value = null;
  notice.value = null;
  try {
    if (operation === "disconnect") await disconnect(ws, connector);
    else if (operation === "sync") await sync(ws, connector);
    else if (reference) {
      const targetRow = await lookupTarget(ws, target, targetType);
      if (!sameActor(captured, capturedEpoch)) return;
      await link(ws, reference.id, {
        documentId: targetType === "document" ? targetRow.id : null,
        taskId: targetType === "task" ? targetRow.id : null,
        anchor: null,
        expectedVersion: reference.localVersion,
      });
    }
    if (!sameActor(captured, capturedEpoch)) return;
    await refresh(captured, ws, connector);
    if (selected.value === connector && operation === "link") {
      notice.value = t("zotero.linked");
      targetId.value = "";
      linkReference.value = null;
    }
    disconnectOpen.value = false;
  } catch (err) {
    if (sameActor(captured, capturedEpoch) && selected.value === connector) error.value = fail(err);
    if (sameActor(captured, capturedEpoch)) await refresh(captured, ws, connector);
  } finally {
    if (sameActor(captured, capturedEpoch)) busy.value = false;
  }
}
function availability(reference: Reference): string {
  if (reference.availability === "deleted") return t("zotero.deleted");
  if (reference.availability === "trashed") return t("zotero.trashed");
  if (reference.availability === "excluded") return t("zotero.excluded");
  return t("zotero.available");
}
</script>

<template>
  <section
    class="settings-section zotero-section"
    :aria-labelledby="`${formId}-title`"
    data-testid="zotero-section"
  >
    <h2 :id="`${formId}-title`" class="settings-section__title text-title">{{
      t("zotero.title")
    }}</h2>
    <p>{{ t("zotero.description") }}</p>
    <p class="text-muted">{{ t("zotero.private") }}</p>
    <p v-if="error" role="alert">{{ error }}</p>
    <p v-if="notice" role="status">{{ notice }}</p>
    <form class="zotero-form" @submit.prevent="onConnect">
      <label :for="`${formId}-kind`">{{ t("zotero.libraryType") }}</label>
      <USelect :id="`${formId}-kind`" v-model="kind" :items="typeOptions" :disabled="busy" />
      <label :for="`${formId}-library`">{{ t("zotero.libraryId") }}</label>
      <UInput
        :id="`${formId}-library`"
        v-model="remoteId"
        inputmode="numeric"
        :disabled="busy"
        autocomplete="off"
        required
      />
      <label :for="`${formId}-url`">{{ t("zotero.libraryUrl") }}</label>
      <UInput
        :id="`${formId}-url`"
        v-model="libraryUrl"
        type="url"
        :disabled="busy"
        autocomplete="off"
        required
      />
      <label :for="`${formId}-key`">{{ t("zotero.key") }}</label>
      <UInput
        :id="`${formId}-key`"
        v-model="secret"
        type="password"
        :disabled="busy"
        autocomplete="new-password"
        required
      />
      <UButton type="submit" :loading="busy" :disabled="busy">{{ t("zotero.connect") }}</UButton>
    </form>
    <p v-if="workspaces.error.value || connectors.error.value" role="alert">{{
      fail(workspaces.error.value || connectors.error.value)
    }}</p>
    <template v-if="connectorOptions.length">
      <label :for="`${formId}-choose`">{{ t("zotero.choose") }}</label>
      <USelect
        :id="`${formId}-choose`"
        v-model="selected"
        :items="connectorOptions"
        :disabled="busy"
      />
      <div class="zotero-actions">
        <UButton
          :disabled="busy || library.data.value?.connector.state !== 'connected'"
          @click="action('sync')"
          >{{ t("zotero.sync") }}</UButton
        >
        <UButton
          variant="outline"
          :disabled="busy || library.data.value?.connector.state === 'disconnected'"
          @click="disconnectOpen = true"
          >{{ t("zotero.disconnect") }}</UButton
        >
      </div>
      <p v-if="library.error.value" role="alert">{{ fail(library.error.value) }}</p>
      <template v-if="library.data.value">
        <p>{{ t("zotero.completed") }}: {{ library.data.value.connector.completedVersion }}</p>
        <p v-if="library.data.value.connector.progressVersion" role="status">{{
          t("zotero.partial")
        }}</p>
        <p v-if="library.data.value.connector.retryAt"
          >{{ t("zotero.retryAt") }}: {{ library.data.value.connector.retryAt }}</p
        >
        <p v-if="library.data.value.connector.state === 'disconnected'">{{
          t("zotero.disconnected")
        }}</p>
        <p v-if="library.data.value.connector.state === 'denied'" role="alert">{{
          t("zotero.denied")
        }}</p>
        <p v-if="!library.data.value.references.length">{{ t("zotero.empty") }}</p>
        <ul class="zotero-references">
          <li
            v-for="reference in library.data.value.references"
            :key="reference.id"
            :data-reference-id="reference.id"
          >
            <h3>{{ reference.bibliography.title }}</h3>
            <p>{{
              reference.bibliography.creators
                .map(
                  (creator) =>
                    creator.name || [creator.firstName, creator.lastName].filter(Boolean).join(" "),
                )
                .join(", ")
            }}</p>
            <p
              >{{ reference.bibliography.fields.date }}
              <span class="text-muted">{{ availability(reference) }}</span></p
            >
            <p v-if="reference.collectionKeys.length"
              >{{ t("zotero.collections") }}:
              {{
                reference.collectionKeys
                  .map(
                    (key) =>
                      library.data.value?.collections.find((collection) => collection.key === key)
                        ?.name || key,
                  )
                  .join(", ")
              }}</p
            >
            <div class="zotero-actions">
              <a :href="documentPath(workspaceSlug, reference.documentDisplayId)">{{
                t("zotero.referenceNote")
              }}</a>
              <a :href="reference.returnUrl" target="_blank" rel="noopener noreferrer">{{
                t("zotero.return")
              }}</a>
              <UButton
                variant="outline"
                :disabled="busy"
                @click="
                  linkReference = reference;
                  targetId = '';
                "
                >{{ t("zotero.link") }}</UButton
              >
            </div>
            <ul v-if="reference.links.length"
              ><li v-for="edge in reference.links" :key="edge.documentId || edge.taskId || ''"
                ><a
                  :href="
                    edge.documentId
                      ? documentPath(workspaceSlug, edge.displayId)
                      : itemPath(workspaceSlug, edge.displayId)
                  "
                  >{{ edge.displayId }}</a
                ></li
              ></ul
            >
          </li>
        </ul>
        <form v-if="linkReference" class="zotero-form" @submit.prevent="action('link')">
          <h3>{{ linkReference.bibliography.title }}</h3>
          <label :for="`${formId}-target-kind`">{{ t("zotero.targetKind") }}</label>
          <USelect
            :id="`${formId}-target-kind`"
            v-model="targetKind"
            :items="targetOptions"
            :disabled="busy"
          />
          <label :for="`${formId}-target`">{{ t("zotero.targetId") }}</label>
          <UInput :id="`${formId}-target`" v-model="targetId" :disabled="busy" required />
          <UButton type="submit" :disabled="busy">{{ t("zotero.link") }}</UButton>
        </form>
      </template>
    </template>
    <ConfirmDialog
      :open="disconnectOpen"
      :title="t('zotero.disconnect')"
      :body="t('zotero.disconnectPrompt')"
      :action-label="t('zotero.disconnect')"
      :pending="busy"
      @close="disconnectOpen = false"
      @confirm="action('disconnect')"
    />
  </section>
</template>

<style scoped>
.zotero-section {
  font-size: 1rem;
  line-height: 1.6;
  overflow-wrap: anywhere;
}
.zotero-form {
  display: grid;
  gap: 0.5rem;
  margin-block: 1rem;
  max-width: 32rem;
}
.zotero-form label {
  font-size: 0.875rem;
  line-height: 1.5;
}
.zotero-actions {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 0.75rem;
  margin-block: 0.75rem;
}
.zotero-references {
  list-style: none;
  padding: 0;
}
.zotero-references > li {
  padding-block: 1rem;
  border-bottom: 1px solid var(--ui-border);
}
</style>
