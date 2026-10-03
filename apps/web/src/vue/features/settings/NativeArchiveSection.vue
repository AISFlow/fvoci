<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, nextTick, onScopeDispose, ref, useTemplateRef, watch } from "vue";
import type { components } from "@/generated/api";
import { projectsQuery } from "@/features/projects/queries";
import { api, ensureOk, loadErrorMessage, ProblemError } from "@/lib/api";
import { meQuery } from "@/lib/queries";
import { IMPORT_POLL_MS, IMPORT_POLL_TRIES, pollImportJob } from "@/lib/import-poll";
import QueryError from "../../components/QueryError.vue";
import { ArchiveLifetime, canConfirmNativeArchive, readNativeArchive } from "./native-archive";

const props = defineProps<{
  workspaceId: string;
  canManage: boolean;
  mode: "export" | "restore";
}>();
const me = useQuery(meQuery);
const client = useQueryClient();
const projects = useQuery(() => ({
  ...projectsQuery(props.workspaceId),
  enabled: props.canManage && props.mode === "export",
}));
const projectId = ref("");
const pending = ref(false);
const error = ref<string | null>(null);
const message = ref<string | null>(null);
const unsupportedDetail = ref<string | null>(null);
const pickerOpen = ref(false);
const fileInput = useTemplateRef<HTMLInputElement>("native-file");
const confirmed = ref(false);
const lifetime = new ArchiveLifetime();
type Preflight = components["schemas"]["NativePreflightOutput"];
type Capture = ReturnType<ArchiveLifetime["capture"]>;
const selection = ref<{
  capture: Capture;
  archiveBase64: string;
  preflight: Preflight;
  requestId: string;
  jobId?: string;
} | null>(null);

function clear(): void {
  lifetime.reset();
  selection.value = null;
  confirmed.value = false;
  projectId.value = "";
  pending.value = false;
  error.value = null;
  message.value = null;
  unsupportedDetail.value = null;
  pickerOpen.value = false;
}
watch(
  [
    () => props.workspaceId,
    () => props.canManage,
    () => props.mode,
    () => me.data.value?.userId,
    () => me.data.value?.sessionId,
  ],
  clear,
  { flush: "sync" },
);
onScopeDispose(() => {
  lifetime.reset();
});
const ready = computed(() => props.canManage && Boolean(me.data.value?.userId));
function capture(): Capture | null {
  if (!ready.value || !me.data.value) return null;
  return lifetime.capture(props.workspaceId, me.data.value.userId, me.data.value.sessionId);
}
function fail(err: unknown, operation: Capture): void {
  if (!operation.current()) return;
  error.value =
    err instanceof ProblemError && err.reason === "native_archive_unsupported"
      ? t("nativeArchive.unsupported")
      : err instanceof ProblemError && err.status === 409
        ? t("nativeArchive.conflict")
        : loadErrorMessage(err);
}

async function archiveResult<T>(
  result: Parameters<typeof ensureOk<T>>[0],
  operation: Capture,
): Promise<T> {
  const params = result.error?.params;
  if (
    operation.current() &&
    typeof params === "object" &&
    params !== null &&
    "code" in params &&
    params.code === "native_archive_unsupported" &&
    "diagnostic" in params &&
    typeof params.diagnostic === "string"
  ) {
    unsupportedDetail.value = params.diagnostic;
  }
  return ensureOk(result);
}

async function download(): Promise<void> {
  const operation = capture();
  const selectedProjectId = projectId.value;
  if (!operation || !selectedProjectId || pending.value) return;
  pending.value = true;
  error.value = null;
  unsupportedDetail.value = null;
  try {
    const blob = await archiveResult(
      await api.GET("/api/v1/workspaces/{workspace_id}/projects/{project_id}/native-archive", {
        params: { path: { workspace_id: operation.workspaceId, project_id: selectedProjectId } },
        signal: operation.signal,
        parseAs: "blob",
      }),
      operation,
    );
    if (!operation.current()) return;
    const href = URL.createObjectURL(blob);
    const link = document.createElement("a");
    link.href = href;
    link.download = "fvoci-native-project.zip";
    link.rel = "noopener";
    try {
      document.body.appendChild(link);
      link.click();
    } finally {
      link.remove();
      URL.revokeObjectURL(href);
    }
  } catch (err) {
    fail(err, operation);
  } finally {
    if (operation.current()) pending.value = false;
  }
}

async function choose(event: Event): Promise<void> {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  input.value = "";
  pickerOpen.value = false;
  if (!file || !ready.value || pending.value) return;
  clear();
  const operation = capture();
  if (!operation) return;
  pending.value = true;
  try {
    const archiveBase64 = await readNativeArchive(file, operation.signal);
    const preflight = await archiveResult(
      await api.POST("/api/v1/workspaces/{workspace_id}/native-archive/preflight", {
        params: { path: { workspace_id: operation.workspaceId } },
        body: { archiveBase64 },
        signal: operation.signal,
      }),
      operation,
    );
    if (!operation.current()) return;
    if (!canConfirmNativeArchive(preflight, operation.workspaceId, operation.actorId)) {
      error.value = t("nativeArchive.unsupported");
      return;
    }
    selection.value = {
      capture: operation,
      archiveBase64,
      preflight,
      requestId: crypto.randomUUID(),
    };
  } catch (err) {
    fail(err, operation);
  } finally {
    if (operation.current()) pending.value = false;
  }
}

async function chooseFile(): Promise<void> {
  const operation = capture();
  if (!operation || pending.value) return;
  pickerOpen.value = true;
  await nextTick();
  if (operation.current()) fileInput.value?.click();
}

async function restore(): Promise<void> {
  const selected = selection.value;
  if (!selected || !selected.capture.current() || !ready.value || pending.value || !confirmed.value)
    return;
  const operation = selected.capture;
  pending.value = true;
  error.value = null;
  unsupportedDetail.value = null;
  message.value = null;
  try {
    if (!selected.jobId) {
      // A response loss retains this exact request ID/hash/target. Retrying
      // reauthorizes the existing durable command instead of creating a graph twice.
      const job = await archiveResult(
        await api.POST("/api/v1/workspaces/{workspace_id}/native-archive/restore", {
          params: { path: { workspace_id: operation.workspaceId } },
          body: {
            archiveBase64: selected.archiveBase64,
            archiveHash: selected.preflight.archiveHash,
            requestId: selected.requestId,
            destinationActorId: operation.actorId,
            confirm: true,
          },
          signal: operation.signal,
        }),
        operation,
      );
      if (!operation.current()) return;
      selected.jobId = job.id;
    }
    const jobId = selected.jobId;
    const result = await pollImportJob({
      signal: operation.signal,
      intervalMs: IMPORT_POLL_MS,
      maxTries: IMPORT_POLL_TRIES,
      fetchStatus: async (signal) =>
        ensureOk(
          await api.GET("/api/v1/workspaces/{workspace_id}/native-archive/jobs/{job_id}", {
            params: { path: { workspace_id: operation.workspaceId, job_id: jobId } },
            signal,
          }),
        ),
    });
    if (!operation.current()) return;
    if (result.kind === "completed") {
      message.value = t("nativeArchive.done");
      selection.value = null;
      confirmed.value = false;
      await client.invalidateQueries({ queryKey: ["projects", operation.workspaceId] });
    } else if (result.kind === "failed") error.value = t("nativeArchive.failed");
    else message.value = t("nativeArchive.resume");
  } catch (err) {
    fail(err, operation);
  } finally {
    if (operation.current()) pending.value = false;
  }
}
</script>

<template>
  <section v-if="canManage" class="settings-section native-archive">
    <h2 class="settings-section__title">{{ t("nativeArchive.title") }}</h2>
    <p>{{ t("nativeArchive.scope") }}</p>
    <p>{{ t("nativeArchive.collisionPolicy") }}</p>
    <p>{{ t("nativeArchive.integrity") }}</p>
    <QueryError
      v-if="me.isError.value"
      :message="loadErrorMessage(me.error.value)"
      @retry="me.refetch()"
    />
    <template v-if="mode === 'export'">
      <QueryError
        v-if="projects.isError.value"
        :message="loadErrorMessage(projects.error.value)"
        @retry="projects.refetch()"
      />
      <label
        >{{ t("nativeArchive.project") }}
        <select v-model="projectId" class="native-archive__select" :disabled="pending || !ready">
          <option value="">{{ t("nativeArchive.chooseProject") }}</option>
          <option
            v-for="project in projects.data.value?.items ?? []"
            :key="project.id"
            :value="project.id"
          >
            {{ project.name }}
          </option>
        </select>
      </label>
      <UButton :disabled="!ready || !projectId || pending" :loading="pending" @click="download">
        {{ t("nativeArchive.export") }}
      </UButton>
    </template>
    <template v-else>
      <UButton type="button" :disabled="!ready || pending" @click="chooseFile">{{
        t("nativeArchive.chooseFile")
      }}</UButton>
      <label v-if="pickerOpen"
        >{{ t("nativeArchive.chooseFile") }}
        <input
          ref="native-file"
          type="file"
          accept=".zip,application/zip"
          :disabled="!ready || pending"
          @change="choose"
          @cancel="pickerOpen = false"
        />
      </label>
      <div v-if="selection" class="native-archive__summary">
        <p>{{ selection.preflight.projectName }}</p>
        <dl>
          <dt>{{ t("nativeArchive.sourceWorkspace") }}</dt
          ><dd>{{ selection.preflight.sourceWorkspaceId }}</dd>
          <dt>{{ t("nativeArchive.destinationWorkspace") }}</dt
          ><dd>{{ selection.preflight.destinationWorkspaceId }}</dd>
          <dt>{{ t("nativeArchive.destinationActor") }}</dt
          ><dd>{{ selection.preflight.destinationActorId }}</dd>
          <dt>{{ t("nativeArchive.archiveHash") }}</dt
          ><dd>{{ selection.preflight.archiveHash }}</dd> <dt>{{ t("nativeArchive.documents") }}</dt
          ><dd>{{ selection.preflight.documentCount }}</dd> <dt>{{ t("nativeArchive.tasks") }}</dt
          ><dd>{{ selection.preflight.taskCount }}</dd> <dt>{{ t("nativeArchive.attachments") }}</dt
          ><dd>{{ selection.preflight.attachmentCount }}</dd>
          <dt>{{ t("nativeArchive.revisions") }}</dt
          ><dd>{{ selection.preflight.revisionCount }}</dd>
        </dl>
        <p>{{ t("nativeArchive.attribution") }}</p>
        <label
          ><input v-model="confirmed" type="checkbox" :disabled="pending" />
          {{ t("nativeArchive.confirm") }}</label
        >
        <UButton :disabled="!confirmed || !ready || pending" :loading="pending" @click="restore">
          {{ selection.jobId ? t("nativeArchive.resumeAction") : t("nativeArchive.restore") }}
        </UButton>
      </div>
    </template>
    <p v-if="message" role="status">{{ message }}</p>
    <p v-if="error" role="alert" class="settings-notice settings-notice--danger">{{ error }}</p>
    <p v-if="unsupportedDetail" role="alert" class="settings-notice settings-notice--danger">{{
      unsupportedDetail
    }}</p>
  </section>
</template>

<style scoped>
.native-archive {
  display: grid;
  gap: 0.75rem;
  font-size: 1rem;
  line-height: 1.6;
  word-break: keep-all;
  overflow-wrap: anywhere;
}
.native-archive label {
  display: grid;
  gap: 0.5rem;
}
.native-archive__select {
  min-height: 2.75rem;
  border: 1px solid var(--ui-border);
  border-radius: 0.5rem;
  padding: 0.5rem 0.75rem;
  background: var(--ui-bg);
}
.native-archive__summary {
  display: grid;
  gap: 0.75rem;
}
.native-archive dl {
  display: grid;
  gap: 0.25rem;
}
.native-archive dt {
  font-weight: 600;
}
.native-archive dd {
  margin: 0 0 0.5rem;
}
</style>
