<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useMutation, useQuery } from "@tanstack/vue-query";
import { computed, onUnmounted, ref, useTemplateRef } from "vue";
import { projectsQuery } from "@/features/projects/queries";
import { ProblemError } from "@/lib/api";
import {
  fetchImportStatus,
  IMPORT_POLL_MS,
  IMPORT_POLL_TRIES,
  isImportActive,
  pollImportJob,
  type ImportJobStatus,
} from "@/lib/import-poll";
import { IMPORT_ACCEPT, IMPORT_NO_PROJECT, IMPORT_SOURCES, type ImportSource } from "./import-source";
import "@/features/settings/settings-shell.css";

class PollStopped extends Error {
  readonly kind: "cancelled" | "budget";
  constructor(kind: "cancelled" | "budget") {
    super(`import poll ${kind}`);
    this.kind = kind;
  }
}

const props = defineProps<{ workspaceId: string; canManage: boolean }>();
const inputRef = useTemplateRef<HTMLInputElement>("file-input");
const pollRef = ref<AbortController | null>(null);
const source = ref<ImportSource>("markdown-zip");
const projectId = ref(IMPORT_NO_PROJECT);
const message = ref<string | null>(null);
const error = ref<string | null>(null);
const resumeJobId = ref<string | null>(null);

const projects = useQuery(() => ({
  ...projectsQuery(props.workspaceId),
  enabled: props.canManage && source.value === "notion-zip",
}));

onUnmounted(() => pollRef.value?.abort());

function fileToBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const result = reader.result;
      if (typeof result !== "string") {
        reject(new Error("read failed"));
        return;
      }
      const comma = result.indexOf(",");
      resolve(comma >= 0 ? result.slice(comma + 1) : result);
    };
    reader.onerror = () => reject(reader.error ?? new Error("read failed"));
    reader.readAsDataURL(file);
  });
}

async function waitForJob(jobId: string): Promise<void> {
  pollRef.value?.abort();
  const controller = new AbortController();
  pollRef.value = controller;
  resumeJobId.value = null;
  try {
    const outcome = await pollImportJob({
      fetchStatus: (signal) => fetchImportStatus(props.workspaceId, jobId, signal),
      signal: controller.signal,
      intervalMs: IMPORT_POLL_MS,
      maxTries: IMPORT_POLL_TRIES,
    });
    if (outcome.kind === "failed") throw new ProblemError(400, "import_failed");
    if (outcome.kind === "cancelled" || outcome.kind === "budget") {
      resumeJobId.value = jobId;
      throw new PollStopped(outcome.kind);
    }
  } catch (err) {
    // Keep the durable job available after a transient status-read failure.
    // Resuming observes the existing job instead of submitting the file twice.
    if (!(err instanceof ProblemError) && !controller.signal.aborted) resumeJobId.value = jobId;
    throw err;
  } finally {
    if (pollRef.value === controller) pollRef.value = null;
  }
}

function showDone(): void {
  message.value = t("workspace.import.ok");
  error.value = null;
}

function showError(err: Error): void {
  if (err instanceof PollStopped) {
    error.value = null;
    message.value = err.kind === "budget" ? t("workspace.import.timeout") : null;
    return;
  }
  message.value = null;
  error.value = err instanceof ProblemError ? err.title : t("workspace.import.failed");
}

const importMutation = useMutation({
  mutationFn: async (file: File) => {
    const workspaceId = props.workspaceId;
    const selectedSource = source.value;
    const selectedProjectId = projectId.value;
    const zipBase64 = await fileToBase64(file);
    const response = await fetch("/api/v1/import", {
      method: "POST",
      credentials: "include",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        workspaceId,
        source: selectedSource,
        zipBase64,
        fileName: file.name,
        ...(selectedSource === "notion-zip" && selectedProjectId !== IMPORT_NO_PROJECT
          ? { projectId: selectedProjectId }
          : {}),
      }),
    });
    if (!response.ok) throw new ProblemError(response.status, "import_failed");
    const job = (await response.json()) as { id: string; status: ImportJobStatus };
    if (isImportActive(job.status)) await waitForJob(job.id);
    else if (job.status !== "completed") throw new ProblemError(400, "import_failed");
    return job;
  },
  onSuccess: showDone,
  onError: showError,
});

const resumeMutation = useMutation({
  mutationFn: waitForJob,
  onSuccess: showDone,
  onError: showError,
});

const pending = computed(() => importMutation.isPending.value || resumeMutation.isPending.value);
const accept = computed(() => IMPORT_ACCEPT[source.value]);

function onFileChange(event: Event): void {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  input.value = "";
  if (!file) return;
  if (pending.value || !props.canManage) return;
  message.value = null;
  error.value = null;
  resumeJobId.value = null;
  importMutation.mutate(file);
}

function sourceLabel(value: ImportSource): string {
  if (value === "markdown-zip") return t("workspace.import.source.markdown-zip");
  if (value === "office-file") return t("workspace.import.source.office-file");
  return t("workspace.import.source.notion-zip");
}
</script>

<template>
  <section v-if="canManage" class="settings-section">
    <h2 class="settings-section__title">{{ t("workspace.import.source") }}</h2>
    <div class="flex flex-col gap-2">
      <label for="import-source">{{ t("workspace.import.source") }}</label>
      <select
        id="import-source"
        v-model="source"
        class="h-11 rounded-md border border-default bg-default px-3"
        :disabled="pending"
      >
        <option v-for="item in IMPORT_SOURCES" :key="item" :value="item">{{ sourceLabel(item) }}</option>
      </select>
      <template v-if="source === 'notion-zip'">
        <label for="import-project">{{ t("workspace.import.project") }}</label>
        <select
          id="import-project"
          v-model="projectId"
          class="h-11 rounded-md border border-default bg-default px-3"
          :disabled="pending"
        >
          <option :value="IMPORT_NO_PROJECT">{{ t("workspace.import.project.none") }}</option>
          <option v-for="project in projects.data.value?.items ?? []" :key="project.id" :value="project.id">
            {{ project.name }}
          </option>
        </select>
      </template>
      <input ref="file-input" type="file" :accept="accept" class="hidden" @change="onFileChange" />
      <UButton type="button" :disabled="pending" @click="inputRef?.click()">
        {{ pending ? t("workspace.import.running") : t("workspace.import.source") }}
      </UButton>
      <UButton v-if="pending" type="button" variant="outline" color="neutral" @click="pollRef?.abort()">
        {{ t("workspace.import.cancelPoll") }}
      </UButton>
      <UButton
        v-if="!pending && resumeJobId"
        type="button"
        variant="outline"
        color="neutral"
        @click="resumeMutation.mutate(resumeJobId)"
      >
        {{ t("workspace.import.resumePoll") }}
      </UButton>
      <p v-if="message" role="status" class="text-muted">{{ message }}</p>
      <p v-if="error" role="alert" class="text-error">{{ error }}</p>
    </div>
  </section>
</template>
