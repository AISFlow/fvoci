<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, onScopeDispose, ref, useId, watch } from "vue";
import {
  createOriginProject,
  createTaskFromDocument,
  documentTaskProjectsQuery,
  originErrorText,
  taskOriginsQuery,
} from "@/features/collections/origin-api";
import { originCreateSurface } from "@/features/collections/origin-create-surface";
import NativeModal from "../../components/NativeModal.vue";
import { ProblemError } from "@/lib/api";
import { meQuery, workspacesQuery } from "@/lib/queries";
import { invalidateTaskCaches } from "@/features/tasks/task-cache";
import { originHref } from "../capture/source-block";
import {
  forgetCommand,
  inputScope,
  recoverCommand,
  rememberCommand,
  type PendingInputCommand,
} from "../capture/capture-command";
import { createPersonalInput } from "../capture/personal-input-api";

// Tasks this document started, or the documents a task came from
// (features/collections/origin-panel.tsx). Create-task lives on the document
// page only. Links to the other app are full page loads.
const props = defineProps<{
  workspaceId: string;
  slug: string;
  documentId?: string;
  taskId?: string;
  hideWhenEmpty?: boolean;
  sourceSelection?: () => { anchor: string; title: string } | null;
  waitForSave?: () => Promise<void>;
}>();
const queryClient = useQueryClient();
const me = useQuery(meQuery);
const workspaces = useQuery(workspacesQuery);
const personal = computed(
  () =>
    workspaces.data.value?.items.some(
      (item) => item.id === props.workspaceId && item.kind === "personal",
    ) ?? false,
);
const anchor = ref<string>();
const pendingCommand = ref<PendingInputCommand | null>(null);
const scope = inputScope();
const confirmOpen = ref(false);
const confirmId = useId();
const namespace = (ws: string, document: string) => `origin:${ws}:${document}`;
const selectionError = ref<string>();
onScopeDispose(() => {
  scope.retire();
});
const after = ref<string | null>(null);
const projectId = ref("");
const title = ref("");
const requestId = ref<string>(crypto.randomUUID());
const projectName = ref("");
const projectKey = ref("");

const origins = useQuery(() =>
  taskOriginsQuery(
    props.workspaceId,
    { documentId: props.documentId, taskId: props.taskId },
    after.value,
  ),
);
const projects = useQuery(() => documentTaskProjectsQuery(props.workspaceId, props.documentId));
const hide = computed(
  () =>
    props.hideWhenEmpty &&
    !origins.isLoading.value &&
    !origins.isError.value &&
    origins.data.value?.count === 0,
);
const heading = computed(() =>
  props.documentId ? t("collection.linkedTasks") : t("collection.sourceDocument"),
);

watch(
  [() => projects.data.value, projectId],
  ([data]) => {
    if (!data) return;
    if (!data.items.some((item) => item.id === projectId.value))
      projectId.value = data.suggestedId ?? "";
  },
  { immediate: true },
);

type Operation = {
  command: PendingInputCommand;
  captured: ReturnType<typeof scope.capture>;
  personal: boolean;
  save?: () => Promise<void>;
};
const createTask = useMutation({
  mutationFn: async (operation: Operation) => {
    if (!scope.sameActor(operation.captured)) throw new Error(t("capture.unavailable"));
    const { command } = operation;
    const document = command.body.source?.documentId;
    const project = command.body.projectId;
    if (!document || (!project && !operation.personal)) throw new Error(t("capture.unavailable"));
    if (command.body.source?.anchor && !operation.save) throw new Error(t("collab unavailable"));
    rememberCommand(window.sessionStorage, command, namespace(command.workspaceId, document));
    await operation.save?.();
    if (!scope.sameActor(operation.captured)) throw new Error(t("capture.unavailable"));
    if (operation.personal) return createPersonalInput(command.workspaceId, command.body);
    if (!project) throw new Error(t("capture.unavailable"));
    const result = await createTaskFromDocument(command.workspaceId, document, {
      projectId: project,
      requestId: command.body.requestId,
      title: command.body.title,
      anchor: command.body.source?.anchor ?? undefined,
    });
    return { ...result, projectId: project };
  },
  onSuccess: async (saved, operation) => {
    if (!scope.sameActor(operation.captured) || !saved.taskId) return;
    const { command } = operation;
    const document = command.body.source?.documentId;
    forgetCommand(window.sessionStorage, command, namespace(command.workspaceId, document ?? ""));
    await invalidateTaskCaches(
      queryClient,
      command.workspaceId,
      saved.projectId ?? command.body.projectId ?? "",
      saved.taskId,
      document,
    );
    if (!scope.current(operation.captured)) return;
    after.value = null;
    title.value = "";
    anchor.value = undefined;
    pendingCommand.value = null;
    requestId.value = crypto.randomUUID();
  },
});
watch(
  [
    () => props.workspaceId,
    () => props.documentId,
    () => props.taskId,
    () => me.data.value?.userId ?? "",
    () => me.data.value?.sessionId ?? "",
    () => me.error.value instanceof ProblemError && me.error.value.status === 401,
  ],
  ([ws, document, task, actor, credential, retired]) => {
    scope.bind(retired ? "" : actor, `${ws}:${document ?? task ?? ""}`, credential);
    title.value = "";
    anchor.value = undefined;
    after.value = null;
    confirmOpen.value = false;
    selectionError.value = undefined;
    pendingCommand.value =
      document && actor
        ? recoverCommand(window.sessionStorage, actor, namespace(ws, document))
        : null;
    if (pendingCommand.value) {
      title.value = pendingCommand.value.body.title;
      projectId.value = pendingCommand.value.body.projectId ?? "";
      anchor.value = pendingCommand.value.body.source?.anchor ?? undefined;
      requestId.value = pendingCommand.value.body.requestId;
    } else requestId.value = crypto.randomUUID();
  },
  { immediate: true, flush: "sync" },
);

const createProject = useMutation({
  mutationFn: (operation: {
    workspaceId: string;
    documentId?: string;
    key: string;
    name: string;
    captured: ReturnType<typeof scope.capture>;
  }) => {
    if (!scope.sameActor(operation.captured)) throw new Error(t("capture.unavailable"));
    return createOriginProject(operation.workspaceId, operation.key, operation.name);
  },
  onSuccess: async (project, operation) => {
    if (!scope.sameActor(operation.captured)) return;
    await Promise.all([
      queryClient.invalidateQueries({
        queryKey: ["task-projects", operation.workspaceId, operation.documentId],
      }),
      queryClient.invalidateQueries({ queryKey: ["projects", operation.workspaceId] }),
    ]);
    if (scope.current(operation.captured)) {
      projectId.value = project.id;
      requestId.value = crypto.randomUUID();
      projectName.value = "";
      projectKey.value = "";
    }
  },
});

const surface = computed(() =>
  personal.value
    ? "create-task"
    : originCreateSurface({
        isLoading: projects.isLoading.value,
        isError: projects.isError.value,
        itemCount: projects.data.value?.items.length,
        canCreateProject: projects.data.value?.canCreateProject,
      }),
);

function preventImeSubmit(event: KeyboardEvent): void {
  if (event.key === "Enter" && event.isComposing) event.preventDefault();
}

// A changed request is a new request: the id makes a retried submit idempotent.
function onProjectChange(event: Event): void {
  projectId.value = (event.target as HTMLSelectElement).value;
  requestId.value = crypto.randomUUID();
}

function onTitleInput(event: Event): void {
  title.value = (event.target as HTMLInputElement).value;
  requestId.value = crypto.randomUUID();
}

function submitTask(): void {
  if ((!personal.value && !projectId.value) || !title.value.trim() || createTask.isPending.value)
    return;
  if (!personal.value) {
    confirmOpen.value = true;
    return;
  }
  dispatchTask();
}
function dispatchTask(): void {
  const actor = me.data.value?.userId;
  const captured = scope.capture();
  if (!actor || captured.actor !== actor || !props.documentId) return;
  const command = pendingCommand.value ?? {
    actorId: actor,
    workspaceId: props.workspaceId,
    body: {
      requestId: requestId.value,
      intent: "task" as const,
      title: title.value.trim(),
      projectId: projectId.value || undefined,
      source: { documentId: props.documentId, anchor: anchor.value },
    },
  };
  pendingCommand.value = command;
  confirmOpen.value = false;
  createTask.mutate({
    command,
    captured,
    personal: personal.value,
    save: props.waitForSave,
  });
}
function fromBlock(): void {
  const selected = props.sourceSelection?.();
  if (!selected) {
    selectionError.value = t("capture.blockMissing");
    return;
  }
  selectionError.value = undefined;
  title.value = selected.title;
  anchor.value = selected.anchor;
  requestId.value = crypto.randomUUID();
}
function abandonCommand(): void {
  if (
    !pendingCommand.value ||
    createTask.isPending.value ||
    !window.confirm(t("capture.abandonConfirm"))
  )
    return;
  forgetCommand(
    window.sessionStorage,
    pendingCommand.value,
    namespace(props.workspaceId, props.documentId ?? ""),
  );
  pendingCommand.value = null;
  requestId.value = crypto.randomUUID();
}

function submitProject(): void {
  if (projectName.value.trim() && projectKey.value.trim())
    createProject.mutate({
      workspaceId: props.workspaceId,
      documentId: props.documentId,
      key: projectKey.value,
      name: projectName.value,
      captured: scope.capture(),
    });
}

const fieldClass = "h-10 rounded-md border border-default bg-default px-2";
</script>

<template>
  <section
    v-if="!hide"
    :aria-label="heading"
    class="flex flex-col gap-3 rounded-md border border-default p-4"
  >
    <h2 class="text-lg">{{ heading }} ({{ origins.data.value?.count ?? 0 }})</h2>
    <p v-if="origins.isLoading.value" role="status">{{ t("collection.origins.loading") }}</p>
    <p v-if="origins.isError.value" role="alert">{{
      originErrorText(origins.error.value, t("collection.origins.error"))
    }}</p>
    <a
      v-for="item in origins.isError.value ? [] : (origins.data.value?.items ?? [])"
      :key="item.taskId"
      class="text-sm underline"
      :href="
        originHref(
          slug,
          documentId ? item.taskDisplayId : item.documentDisplayId,
          documentId ? null : item.anchor,
        )
      "
    >
      {{
        documentId
          ? `${item.taskDisplayId} · ${item.taskTitle}`
          : `${item.documentDisplayId} · ${item.documentTitle}`
      }}
    </a>
    <p v-if="origins.data.value && origins.data.value.count === 0">{{
      t("collection.noOrigins")
    }}</p>
    <UButton v-if="after" class="w-fit" variant="outline" color="neutral" @click="after = null">
      {{ t("collection.origins.first") }}
    </UButton>
    <UButton
      v-if="origins.data.value?.nextCursor"
      class="w-fit"
      variant="outline"
      color="neutral"
      @click="after = origins.data.value?.nextCursor ?? null"
    >
      {{ t("collection.origins.next") }}
    </UButton>
    <UButton
      color="neutral"
      variant="outline"
      class="w-fit"
      @click="
        origins.refetch();
        projects.refetch();
      "
      >{{ t("capture.recover") }}</UButton
    >
    <div v-if="documentId" class="flex flex-col gap-3">
      <UButton
        v-if="sourceSelection"
        color="neutral"
        variant="outline"
        class="w-fit"
        :disabled="!!pendingCommand || createTask.isPending.value"
        @click="fromBlock"
        >{{ t("capture.fromBlock") }}</UButton
      >
      <p v-if="selectionError" role="alert">{{ selectionError }}</p>
      <p v-if="anchor" class="text-sm">{{ t("capture.sourceBlock") }}: {{ anchor }}</p>
      <p v-if="personal">{{ t("capture.selfAssigned") }}</p>
      <p v-if="pendingCommand" role="status">{{ t("capture.unknown") }}</p>
      <UButton
        v-if="pendingCommand"
        color="neutral"
        variant="ghost"
        :disabled="createTask.isPending.value"
        @click="abandonCommand"
        >{{ t("capture.abandon") }}</UButton
      >
      <p v-if="surface === 'loading'" role="status">{{
        t("collection.taskCreation.projectsLoading")
      }}</p>
      <p v-if="surface === 'error'" role="alert">
        {{ originErrorText(projects.error.value, t("collection.taskCreation.projectsError")) }}
      </p>
      <form
        v-if="surface === 'create-task' && projects.data.value"
        class="flex flex-wrap items-end gap-2"
        @keydown="preventImeSubmit"
        @submit.prevent="submitTask"
      >
        <div class="flex flex-col gap-1">
          <label :for="`origin-project-${documentId}`" class="text-sm font-medium">{{
            t("collection.taskProject")
          }}</label>
          <select
            :id="`origin-project-${documentId}`"
            :class="fieldClass"
            :value="projectId"
            :disabled="!!pendingCommand || createTask.isPending.value"
            @change="onProjectChange"
          >
            <option v-if="personal" value="">{{ t("capture.defaultProject") }}</option>
            <option
              v-for="project in projects.data.value.items"
              :key="project.id"
              :value="project.id"
            >
              {{ project.name }} ({{ project.key }})
            </option>
          </select>
        </div>
        <div class="flex flex-col gap-1">
          <label :for="`origin-title-${documentId}`" class="text-sm font-medium">{{
            t("collection.taskTitle")
          }}</label>
          <input
            :id="`origin-title-${documentId}`"
            :class="fieldClass"
            :value="title"
            :disabled="!!pendingCommand || createTask.isPending.value"
            @input="onTitleInput"
          />
        </div>
        <UButton
          type="submit"
          :disabled="(!personal && !projectId) || !title.trim() || createTask.isPending.value"
        >
          {{ t("collection.createTask") }}
        </UButton>
      </form>
      <form
        v-if="surface === 'create-project'"
        class="flex flex-wrap items-end gap-2"
        @keydown="preventImeSubmit"
        @submit.prevent="submitProject"
      >
        <p class="w-full">{{ t("collection.projectRequired") }}</p>
        <div class="flex flex-col gap-1">
          <label :for="`origin-project-name-${documentId}`" class="text-sm font-medium">{{
            t("collection.projectName")
          }}</label>
          <input
            :id="`origin-project-name-${documentId}`"
            v-model="projectName"
            :class="fieldClass"
          />
        </div>
        <div class="flex flex-col gap-1">
          <label :for="`origin-project-key-${documentId}`" class="text-sm font-medium">{{
            t("project.keyLabel")
          }}</label>
          <input
            :id="`origin-project-key-${documentId}`"
            v-model="projectKey"
            :class="fieldClass"
          />
        </div>
        <UButton
          type="submit"
          :disabled="!projectName.trim() || !projectKey.trim() || createProject.isPending.value"
        >
          {{ t("project.new") }}
        </UButton>
      </form>
      <p v-if="surface === 'unavailable'">{{ t("collection.taskCreation.unavailable") }}</p>
      <p v-if="createTask.isError.value" role="alert">
        {{ originErrorText(createTask.error.value, t("collection.taskCreation.error")) }}
      </p>
      <p v-if="createProject.isError.value" role="alert">
        {{ originErrorText(createProject.error.value, t("collection.taskCreation.projectError")) }}
      </p>
    </div>
    <NativeModal
      :id="confirmId"
      :open="confirmOpen"
      :labelled-by="`${confirmId}-title`"
      dialog-class="mx-auto mt-[15vh] w-[min(32rem,calc(100%-2rem))] rounded-lg border border-default bg-default p-4 text-default backdrop:bg-black/30"
      @close="confirmOpen = false"
    >
      <div class="flex flex-col gap-4 break-keep">
        <h2 :id="`${confirmId}-title`" class="text-xl">{{ t("capture.teamConfirm") }}</h2>
        <p>{{ projects.data.value?.items.find((item) => item.id === projectId)?.name }}</p>
        <p>{{
          projects.data.value?.items.find((item) => item.id === projectId)?.visibility ===
          "workspace"
            ? t("capture.scopeWorkspace")
            : t("capture.scopePrivate")
        }}</p>
        <p class="text-base leading-relaxed">{{ t("capture.teamWarning") }}</p>
        <div class="flex flex-wrap gap-2">
          <UButton @click="dispatchTask">{{ t("capture.confirmCreate") }}</UButton>
          <UButton color="neutral" variant="outline" @click="confirmOpen = false">{{
            t("capture.cancel")
          }}</UButton>
        </div>
      </div>
    </NativeModal>
  </section>
</template>
