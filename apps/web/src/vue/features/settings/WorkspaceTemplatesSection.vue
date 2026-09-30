<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import UInput from "@nuxt/ui/components/Input.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref, useId } from "vue";
import { useRouter } from "vue-router";
import { z } from "zod";
import { projectsQuery } from "@/features/projects/queries";
import type { components } from "@/generated/api";
import { api, ensureOk, ProblemError } from "@/lib/api";
import { itemPath } from "@/lib/href";
import { templatesQuery } from "@/lib/queries";
import QueryLoading from "../../components/QueryLoading.vue";
import { parseForm } from "./form";
import "@/features/settings/settings-shell.css";

type TemplateOutput = components["schemas"]["TemplateOutput"];
type TemplateCreateBody = components["schemas"]["TemplateCreateBody"];

const templateCreateFields = z.object({
  kind: z.enum(["document", "task"]),
  title: z.string().trim().min(1).max(200),
});

const props = defineProps<{ workspaceId: string; slug: string }>();
const router = useRouter();
const queryClient = useQueryClient();
const projectFieldId = useId();
const titleId = useId();
const kindId = useId();
const projectId = ref("none");
const actionError = ref<string | null>(null);
const title = ref("");
const kind = ref<"document" | "task">("document");
const titleError = ref<string | null>(null);

const listQuery = useQuery(() => templatesQuery(props.workspaceId));
const projectsQueryResult = useQuery(() => projectsQuery(props.workspaceId));

function failMessage(err: unknown): string {
  return err instanceof ProblemError ? err.title : t("error.network");
}

async function invalidate(): Promise<void> {
  await queryClient.invalidateQueries({ queryKey: ["templates", props.workspaceId] });
}

const create = useMutation({
  mutationFn: async (input: TemplateCreateBody) =>
    ensureOk(
      await api.POST("/api/v1/workspaces/{workspace_id}/templates", {
        params: { path: { workspace_id: props.workspaceId } },
        body: input,
      }),
    ),
  onSuccess: async () => {
    actionError.value = null;
    title.value = "";
    await invalidate();
  },
  onError: (err: unknown) => {
    actionError.value = failMessage(err);
  },
});

const apply = useMutation({
  mutationFn: async (input: { id: string; kind: TemplateOutput["kind"]; projectId?: string }) =>
    ensureOk(
      await api.POST("/api/v1/workspaces/{workspace_id}/templates/{template_id}/apply", {
        params: { path: { workspace_id: props.workspaceId, template_id: input.id } },
        body:
          input.kind === "task" && input.projectId && input.projectId !== "none"
            ? { projectId: input.projectId }
            : {},
      }),
    ),
  onSuccess: async (applied) => {
    actionError.value = null;
    await router.push(itemPath(props.slug, applied.displayId));
  },
  onError: (err: unknown) => {
    actionError.value = failMessage(err);
  },
});

const pending = computed(() => create.isPending.value || apply.isPending.value);
const templates = computed(() => listQuery.data.value?.items ?? []);
const projects = computed(() => projectsQueryResult.data.value?.items ?? []);
const loading = computed(() => listQuery.isLoading.value);
const error = computed(
  () => actionError.value ?? (listQuery.error.value ? failMessage(listQuery.error.value) : null),
);
const hasTaskTemplate = computed(() => templates.value.some((row) => row.kind === "task"));

function onCreate(): void {
  titleError.value = null;
  const parsed = parseForm(templateCreateFields, { kind: kind.value, title: title.value });
  if (!parsed.ok) {
    titleError.value = parsed.message;
    return;
  }
  create.mutate({
      kind: parsed.data.kind,
      title: parsed.data.title,
      payload: { title: parsed.data.title },
    });
}

function kindLabel(value: TemplateOutput["kind"]): string {
  return value === "task" ? t("template.kind.task") : t("template.kind.document");
}
</script>

<template>
  <div class="settings-stack">
    <section class="settings-section">
      <h2 class="settings-section__title">{{ t("settings.templates") }}</h2>
      <div class="flex flex-col gap-4">
        <form class="flex flex-col gap-2" novalidate @submit.prevent="onCreate">
          <div class="flex flex-col gap-1.5">
            <label :for="titleId">{{ t("template.title") }}</label>
            <UInput :id="titleId" v-model="title" :disabled="pending" :aria-invalid="titleError ? true : undefined" />
            <p v-if="titleError" class="text-error" role="alert">{{ titleError }}</p>
          </div>
          <div class="flex flex-col gap-1.5">
            <label :for="kindId">{{ t("template.kind") }}</label>
            <select
              :id="kindId"
              v-model="kind"
              class="h-11 min-w-28 rounded-md border border-default bg-default px-3"
              :disabled="pending"
              :aria-label="t('template.kind')"
            >
              <option value="document">{{ t("template.kind.document") }}</option>
              <option value="task">{{ t("template.kind.task") }}</option>
            </select>
          </div>
          <UButton type="submit" size="sm" class="w-fit" :disabled="pending">{{ t("template.create") }}</UButton>
        </form>
        <p v-if="error" class="text-error" role="alert">{{ error }}</p>
        <QueryLoading v-if="loading" />
        <p v-if="!loading && templates.length === 0" class="text-muted">{{ t("template.empty") }}</p>
        <div v-if="hasTaskTemplate" class="flex flex-col gap-1.5">
          <label :for="projectFieldId">{{ t("template.project") }}</label>
          <select
            :id="projectFieldId"
            v-model="projectId"
            class="h-11 min-w-28 rounded-md border border-default bg-default px-3"
            :aria-label="t('template.project')"
            :disabled="pending"
          >
            <option value="none">{{ t("template.project.placeholder") }}</option>
            <option v-for="project in projects" :key="project.id" :value="project.id">{{ project.name }}</option>
          </select>
        </div>
        <div v-if="templates.length > 0" class="overflow-x-auto">
          <table class="w-full border-collapse">
            <thead>
              <tr>
                <th scope="col" class="border-b border-default px-2 py-2 text-left">{{ t("template.title") }}</th>
                <th scope="col" class="border-b border-default px-2 py-2 text-left">{{ t("template.kind") }}</th>
                <th scope="col" class="border-b border-default px-2 py-2">
                  <span class="sr-only">{{ t("template.apply") }}</span>
                </th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="row in templates" :key="row.id">
                <td class="border-b border-default px-2 py-2">{{ row.title }}</td>
                <td class="border-b border-default px-2 py-2">{{ kindLabel(row.kind) }}</td>
                <td class="border-b border-default px-2 py-2 text-right">
                  <UButton
                    type="button"
                    size="sm"
                    variant="outline"
                    color="neutral"
                    :disabled="pending || (row.kind === 'task' && projectId === 'none')"
                    @click="
                      apply.mutate({
                        id: row.id,
                        kind: row.kind,
                        projectId: row.kind === 'task' ? projectId : undefined,
                      })
                    "
                  >
                    {{ t("template.apply") }}
                  </UButton>
                </td>
              </tr>
            </tbody>
          </table>
        </div>
      </div>
    </section>
  </div>
</template>
