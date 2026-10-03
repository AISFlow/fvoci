<script setup lang="ts">
import UButton from "@nuxt/ui/components/Button.vue";
import { useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, onScopeDispose, ref, useId, watch } from "vue";
import { api, ensureOk, loadErrorMessage, ProblemError } from "@/lib/api";
import { meQuery } from "@/lib/queries";
import { itemPath, formatDisplayId } from "@/lib/href";
import { myTasksQuery } from "@/features/tasks/my-tasks";
import { explicitMinutes, planDocumentRef, planPresets } from "./task-plan-input";
import { planTargetsQuery, sendPlanTask, type PlanTaskCommand } from "./task-plan-queries";
import {
  captureTimerDenial,
  removeCapturedTimerQuery,
  timerContextChanged,
} from "./task-stopwatch-queries";
import TaskStopwatch from "./TaskStopwatch.vue";

const props = defineProps<{ workspaceId: string; slug: string; personal: boolean }>();
const client = useQueryClient();
const me = useQuery(meQuery);
const actor = computed(() => me.data.value?.userId ?? "");
const session = computed(() => me.data.value?.sessionId ?? "");
const id = useId();
const purpose = ref<string>("study");
const goal = ref("");
const goalMinutes = ref("");
const notesLink = ref("");
const materialLink = ref("");
const projectId = ref("");
const steps = ref<Array<{ title: string; minutes: string }>>(
  planPresets[0].steps.map((title) => ({ title, minutes: "" })),
);
const notesId = ref("");
const materialId = ref("");
const notesAnchor = ref<string | null>(null);
const materialAnchor = ref<string | null>(null);
const notesTargets = useQuery(() =>
  planTargetsQuery(actor.value, session.value, props.workspaceId, notesId.value),
);
const materialTargets = useQuery(() =>
  planTargetsQuery(actor.value, session.value, props.workspaceId, materialId.value),
);
const projects = computed(() => {
  if (
    denied(notesTargets.error.value) ||
    denied(materialTargets.error.value) ||
    !notesTargets.data.value ||
    !materialTargets.data.value
  )
    return [];
  const materialProjects = new Set(materialTargets.data.value.items.map((project) => project.id));
  return notesTargets.data.value.items.filter((project) => materialProjects.has(project.id));
});
const pending = ref(false);
const resolving = ref(false);
const retryable = ref(false);
const error = ref<string>();
const complete = ref(false);
let live = true;
let lifetime = 0;
let generation = 0;
let revision = 0;
type Saved = { id: string; title: string; number: number; projectKey: string };
const saved = ref<Saved[]>([]);
type StepCommand = {
  document: string;
  requestId: string;
  title: string;
  minutes: number | null;
  anchor: string | null;
  body?: PlanTaskCommand;
};
type Capture = {
  actor: string;
  session: string;
  workspace: string;
  notes: string;
  material: string;
  project: string;
  lifetime: number;
  generation: number;
  revision: number;
  selfAssign: boolean;
  nodes: StepCommand[];
  accepted: Saved[];
};
let command: Capture | undefined;
const draftLocked = computed(
  () => pending.value || retryable.value || Boolean(command?.accepted.length),
);
function retireTarget() {
  generation++;
  revision++;
  command = undefined;
  pending.value = false;
  resolving.value = false;
  retryable.value = false;
  error.value = undefined;
  saved.value = [];
  complete.value = false;
}
function linksEdited() {
  retireTarget();
  notesId.value = "";
  materialId.value = "";
  projectId.value = "";
}
function draftEdited() {
  revision++;
  complete.value = false;
  if (!pending.value) {
    command = undefined;
    retryable.value = false;
  }
}
watch(
  () => [actor.value, session.value, props.workspaceId, props.slug],
  () => {
    lifetime++;
    retireTarget();
    notesId.value = "";
    materialId.value = "";
    projectId.value = "";
    notesLink.value = "";
    materialLink.value = "";
    goal.value = "";
    goalMinutes.value = "";
    steps.value = planPresets[0].steps.map((title) => ({ title, minutes: "" }));
  },
  { flush: "sync" },
);
watch(
  projectId,
  () => {
    retireTarget();
  },
  { flush: "sync" },
);
watch(
  () => projects.value.map((project) => project.id).join(","),
  () => {
    if (
      projectId.value &&
      notesTargets.isSuccess.value &&
      materialTargets.isSuccess.value &&
      !projects.value.some((project) => project.id === projectId.value)
    ) {
      retireTarget();
      projectId.value = "";
    }
  },
  { flush: "sync" },
);
onScopeDispose(() => {
  live = false;
  lifetime++;
  generation++;
});
function current(c: Capture) {
  return (
    live &&
    c.actor === actor.value &&
    c.session === session.value &&
    c.workspace === props.workspaceId &&
    c.notes === notesId.value &&
    c.material === materialId.value &&
    c.project === projectId.value &&
    c.lifetime === lifetime &&
    c.generation === generation &&
    c.revision === revision &&
    projects.value.some((project) => project.id === c.project) &&
    !(me.error.value instanceof ProblemError && me.error.value.status === 401)
  );
}
function denied(err: unknown): err is ProblemError {
  return (
    err instanceof ProblemError &&
    ([401, 403, 404].includes(err.status) || timerContextChanged(err))
  );
}
watch(
  () => [notesTargets.error.value, materialTargets.error.value],
  async () => {
    for (const [query, document] of [
      [notesTargets, notesId.value],
      [materialTargets, materialId.value],
    ] as const) {
      if (!denied(query.error.value)) continue;
      const capture = captureTimerDenial(
        client,
        query.error.value,
        planTargetsQuery(actor.value, session.value, props.workspaceId, document).queryKey,
        query.status.value,
        query.fetchStatus.value,
      );
      if (!capture) continue;
      const capturedLifetime = lifetime;
      retireTarget();
      const capturedGeneration = generation;
      error.value = loadErrorMessage(query.error.value);
      const currentDenial = () =>
        live && lifetime === capturedLifetime && generation === capturedGeneration;
      await removeCapturedTimerQuery(client, capture, currentDenial);
      if (currentDenial() && timerContextChanged(query.error.value))
        await client.invalidateQueries({ queryKey: meQuery.queryKey, exact: true });
      if (currentDenial()) {
        notesId.value = "";
        materialId.value = "";
      }
    }
  },
);
async function resolveDocuments() {
  if (pending.value || resolving.value || !actor.value || !session.value) return;
  const notes = planDocumentRef(notesLink.value, props.slug, location.origin);
  const material = planDocumentRef(materialLink.value, props.slug, location.origin);
  if (!notes || !material) {
    error.value = "현재 워크스페이스의 자료·노트 문서 링크를 입력하세요.";
    return;
  }
  const captured = {
    actor: actor.value,
    session: session.value,
    workspace: props.workspaceId,
    lifetime,
    generation,
    revision,
  };
  const stillCurrent = () =>
    live &&
    captured.actor === actor.value &&
    captured.session === session.value &&
    captured.workspace === props.workspaceId &&
    captured.lifetime === lifetime &&
    captured.generation === generation &&
    captured.revision === revision;
  resolving.value = true;
  error.value = undefined;
  try {
    // Lookup titles are never displayed, cached or copied to this planner.
    // Captured project picker GET and every create check authoritative actor.
    const resolve = async (displayId: string) => {
      const result = await ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/lookup/{display_id}", {
          params: { path: { workspace_id: captured.workspace, display_id: displayId } },
        }),
      );
      return result.items.find((item) => item.kind === "document" && item.displayId === displayId)
        ?.id;
    };
    const [n, m] = await Promise.all([resolve(notes.displayId), resolve(material.displayId)]);
    if (!stillCurrent()) return;
    if (!n || !m) {
      error.value =
        "열 수 있는 일반 문서를 선택하세요. 태스크 링크는 자료 문서로 사용할 수 없습니다.";
      return;
    }
    notesAnchor.value = notes.anchor;
    materialAnchor.value = material.anchor;
    notesId.value = n;
    materialId.value = m;
  } catch (err) {
    if (stillCurrent()) error.value = loadErrorMessage(err);
  } finally {
    if (stillCurrent()) resolving.value = false;
  }
}
function applyPreset() {
  if (draftLocked.value) return;
  const preset = planPresets.find((item) => item.id === purpose.value);
  if (preset) {
    steps.value = preset.steps.map((title) => ({ title, minutes: "" }));
    draftEdited();
  }
}
async function submit(c: Capture) {
  if (!current(c) || pending.value) return;
  pending.value = true;
  retryable.value = false;
  error.value = undefined;
  try {
    for (let index = c.accepted.length; index < c.nodes.length; index++) {
      if (!current(c)) return;
      const node = c.nodes[index];
      if (!node) throw new Error("missing captured plan step");
      node.body ??= Object.freeze({
        expectedActorId: c.actor,
        expectedSessionId: c.session,
        requestId: node.requestId,
        projectId: c.project,
        selfAssign: c.selfAssign,
        anchor: node.anchor,
        minutes: node.minutes,
        task: Object.freeze({
          title: node.title,
          type: index === 0 ? "epic" : "task",
          ...(index === 0 ? {} : { parentId: c.accepted[0]?.id }),
        }),
      });
      const result = await sendPlanTask(c.workspace, node.document, node.body);
      if (!current(c)) return;
      c.accepted.push({
        id: result.taskId,
        title: node.title,
        number: result.number,
        projectKey: result.projectKey,
      });
      saved.value = [...c.accepted];
    }
    if (!current(c)) return;
    command = undefined;
    complete.value = true;
    await client.invalidateQueries({ queryKey: myTasksQuery(c.workspace).queryKey, exact: true });
  } catch (err) {
    if (!current(c)) return;
    error.value = loadErrorMessage(err);
    retryable.value = !(err instanceof ProblemError) || err.status === 429 || err.status >= 500;
    if (denied(err)) {
      retireTarget();
      notesId.value = "";
      materialId.value = "";
      error.value = loadErrorMessage(err);
    }
  } finally {
    if (current(c)) pending.value = false;
  }
}
async function createPlan() {
  if (
    draftLocked.value ||
    complete.value ||
    !actor.value ||
    !session.value ||
    !projects.value.some((project) => project.id === projectId.value)
  )
    return;
  const budget = explicitMinutes(goalMinutes.value);
  const rows = steps.value.map((step) => ({
    title: step.title.trim(),
    budget: explicitMinutes(step.minutes),
  }));
  if (
    !goal.value.trim() ||
    Array.from(goal.value.trim()).length > 500 ||
    !budget.valid ||
    rows.some((row) => !row.title || Array.from(row.title).length > 500 || !row.budget.valid)
  ) {
    error.value = "목표와 단계 이름, 0 이상의 정수 예상 시간을 확인하세요.";
    return;
  }
  const nodes: StepCommand[] = [
    {
      document: notesId.value,
      requestId: crypto.randomUUID(),
      title: goal.value.trim(),
      minutes: budget.minutes,
      anchor: notesAnchor.value,
    },
  ];
  for (const row of rows)
    if (row.budget.valid)
      nodes.push({
        document: materialId.value,
        requestId: crypto.randomUUID(),
        title: row.title,
        minutes: row.budget.minutes,
        anchor: materialAnchor.value,
      });
  command = {
    actor: actor.value,
    session: session.value,
    workspace: props.workspaceId,
    notes: notesId.value,
    material: materialId.value,
    project: projectId.value,
    lifetime,
    generation: ++generation,
    revision,
    selfAssign: props.personal,
    nodes,
    accepted: [],
  };
  saved.value = [];
  await submit(command);
}
</script>

<template>
  <details class="min-w-0 rounded-lg border border-default p-4" data-testid="study-plan-builder">
    <summary class="cursor-pointer break-keep font-medium">학습·연구·업무 계획 만들기</summary>
    <p class="mt-3 break-keep text-sm text-muted"
      >일반 프로젝트에 목표와 단계를 만들고, 기존 자료·노트 문서에 연결합니다. 틀을 선택한 뒤 이름과
      시간을 바꿀 수 있습니다.</p
    >
    <form class="mt-4 flex min-w-0 flex-col gap-4" @submit.prevent="createPlan">
      <div class="flex flex-wrap items-end gap-2">
        <div class="flex min-w-0 flex-col gap-1 text-sm">
          <label :for="`${id}-purpose`">계획 틀</label>
          <select
            :id="`${id}-purpose`"
            v-model="purpose"
            :disabled="draftLocked"
            class="rounded-md border border-default bg-default p-2 text-base"
            ><option v-for="preset in planPresets" :key="preset.id" :value="preset.id">{{
              preset.label
            }}</option></select
          >
        </div>
        <UButton
          type="button"
          size="sm"
          variant="outline"
          :disabled="pending || !!command?.accepted.length"
          @click="applyPreset"
          >틀 적용</UButton
        >
      </div>
      <label :for="`${id}-goal`" class="flex min-w-0 flex-col gap-1 text-sm"
        >목표
        <input
          :id="`${id}-goal`"
          v-model="goal"
          maxlength="500"
          :disabled="draftLocked"
          class="min-w-0 rounded-md border border-default bg-default p-2 text-base"
          @input="draftEdited"
        />
      </label>
      <label :for="`${id}-budget`" class="flex min-w-0 flex-col gap-1 text-sm"
        >목표 예상 시간(분, 선택)
        <input
          :id="`${id}-budget`"
          v-model="goalMinutes"
          inputmode="numeric"
          :disabled="draftLocked"
          class="min-w-0 rounded-md border border-default bg-default p-2 text-base"
          @input="draftEdited"
        />
      </label>
      <label :for="`${id}-notes`" class="flex min-w-0 flex-col gap-1 text-sm"
        >목표·연구 노트 문서 링크
        <input
          :id="`${id}-notes`"
          v-model="notesLink"
          :disabled="draftLocked"
          placeholder="WIKI-12 또는 문서 링크"
          class="min-w-0 rounded-md border border-default bg-default p-2 text-base"
          @input="linksEdited"
        />
      </label>
      <label :for="`${id}-material`" class="flex min-w-0 flex-col gap-1 text-sm"
        >읽을 자료 문서 링크
        <input
          :id="`${id}-material`"
          v-model="materialLink"
          :disabled="draftLocked"
          placeholder="WIKI-13 또는 문서 링크"
          class="min-w-0 rounded-md border border-default bg-default p-2 text-base"
          @input="linksEdited"
        />
      </label>
      <UButton
        type="button"
        size="sm"
        class="w-fit"
        :disabled="draftLocked || resolving"
        @click="resolveDocuments"
        >연결할 문서 확인</UButton
      >
      <p
        v-if="notesTargets.isError.value || materialTargets.isError.value"
        role="alert"
        class="break-keep text-sm text-error"
        >{{ loadErrorMessage(notesTargets.error.value ?? materialTargets.error.value) }}</p
      >
      <div v-if="notesId && materialId" class="flex min-w-0 flex-col gap-1 text-sm">
        <label :for="`${id}-project`">저장할 프로젝트</label>
        <select
          :id="`${id}-project`"
          v-model="projectId"
          :disabled="draftLocked"
          class="min-w-0 rounded-md border border-default bg-default p-2 text-base"
          ><option value="">프로젝트를 선택하세요</option
          ><option v-for="project in projects" :key="project.id" :value="project.id"
            >{{ project.key }} · {{ project.name }}</option
          ></select
        >
      </div>
      <fieldset
        v-for="(step, index) in steps"
        :key="index"
        class="flex min-w-0 flex-col gap-2 rounded-md border border-default p-3"
      >
        <legend class="text-sm">단계 {{ index + 1 }}</legend>
        <label :for="`${id}-step-${index}`" class="flex min-w-0 flex-col gap-1 text-sm"
          >단계 이름
          <input
            :id="`${id}-step-${index}`"
            v-model="step.title"
            maxlength="500"
            :disabled="draftLocked"
            class="min-w-0 rounded-md border border-default bg-default p-2 text-base"
            @input="draftEdited"
          />
        </label>
        <label :for="`${id}-minutes-${index}`" class="flex min-w-0 flex-col gap-1 text-sm"
          >단계 예상 시간(분, 선택)
          <input
            :id="`${id}-minutes-${index}`"
            v-model="step.minutes"
            inputmode="numeric"
            :disabled="draftLocked"
            class="min-w-0 rounded-md border border-default bg-default p-2 text-base"
            @input="draftEdited"
          />
        </label>
      </fieldset>
      <div class="flex flex-wrap gap-2">
        <UButton
          type="submit"
          size="sm"
          :disabled="draftLocked || complete || !projectId || !notesId || !materialId"
          >계획 저장</UButton
        >
        <UButton
          v-if="retryable"
          type="button"
          size="sm"
          variant="outline"
          :disabled="pending"
          @click="command && submit(command)"
          >같은 계획 저장 계속하기</UButton
        >
      </div>
      <p v-if="error" role="alert" class="break-keep text-sm text-error">{{ error }}</p>
      <p v-if="saved.length" role="status" class="break-keep text-sm text-muted"
        >{{ saved.length }}개 저장됨. 저장된 목표와 단계는 일반 프로젝트에서도 확인할 수
        있습니다.</p
      >
    </form>
    <ul v-if="saved.length" class="mt-4 flex min-w-0 flex-col gap-3">
      <li v-for="task in saved" :key="task.id" class="min-w-0 rounded-md border border-default p-3">
        <a
          :href="itemPath(slug, formatDisplayId(task.projectKey, task.number))"
          class="break-keep text-sm underline underline-offset-2"
          >{{ task.title }}</a
        >
        <TaskStopwatch :workspace-id="workspaceId" :task-id="task.id" :read-only="false" compact />
      </li>
    </ul>
  </details>
</template>
