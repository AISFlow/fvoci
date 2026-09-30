<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { computed, ref } from "vue";
import { ProblemError } from "@/lib/api";
import { canonicalizeProjectKey } from "@/lib/href";
import type { CloneProjectBody, ProjectListItem } from "@/features/projects/queries";
import type { components } from "@/generated/api";
import LeadSelect from "./LeadSelect.vue";
import { cloneDefaultName, projectFormPayload } from "./project-form";

type Member = components["schemas"]["MemberResponse"];

const props = defineProps<{
  source: ProjectListItem;
  pending?: boolean;
  members: readonly Member[];
  currentUserId: string | null;
  onSubmit: (input: CloneProjectBody) => Promise<void>;
  onCancel: () => void;
}>();

const key = ref("");
const name = ref(cloneDefaultName(props.source.name));
const visibility = ref<"workspace" | "private">(
  props.source.visibility === "private" ? "private" : "workspace",
);
const description = ref(props.source.description ?? "");
const icon = ref(props.source.icon ?? "");
const leadUserId = ref(props.currentUserId ?? "");
const fieldError = ref<string | null>(null);
const serverError = ref<string | null>(null);
const submitting = ref(false);
const busy = computed(() => props.pending || submitting.value);

function onKeyInput(event: Event): void {
  key.value = canonicalizeProjectKey((event.target as HTMLInputElement).value);
}

async function submitForm(event: Event): Promise<void> {
  event.preventDefault();
  fieldError.value = null;
  serverError.value = null;
  const parsed = projectFormPayload({
    key: key.value,
    name: name.value,
    visibility: visibility.value,
    description: description.value,
    icon: icon.value,
    leadUserId: leadUserId.value,
  });
  if (!parsed.ok) {
    fieldError.value = t(parsed.message);
    return;
  }
  const body: CloneProjectBody = {
    key: parsed.body.key,
    name: parsed.body.name,
    visibility: parsed.body.visibility,
    description: parsed.body.description,
    icon: parsed.body.icon,
    ...(parsed.body.leadUserId ? { leadUserId: parsed.body.leadUserId } : {}),
  };
  submitting.value = true;
  try {
    await props.onSubmit(body);
  } catch (err) {
    serverError.value = err instanceof ProblemError ? err.title : t("error.network");
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <form class="project-form" novalidate @submit="submitForm">
    <p class="project-form__hint">{{ t("project.clone.help") }}</p>
    <p class="project-form__hint"
      >{{ t("project.clone.source") }}: {{ source.key }} — {{ source.name }}</p
    >
    <div class="project-form__row">
      <div class="project-form__field">
        <label for="clone-key">{{ t("project.keyLabel") }}</label>
        <input
          id="clone-key"
          :value="key"
          autocomplete="off"
          :disabled="busy"
          @input="onKeyInput"
        />
      </div>
      <div class="project-form__field">
        <label for="clone-name">{{ t("project.name") }}</label>
        <input id="clone-name" v-model="name" :disabled="busy" />
      </div>
      <LeadSelect id="clone-lead" v-model="leadUserId" :members="members" :disabled="busy" />
    </div>
    <p v-if="fieldError" role="alert" class="project-form__alert">{{ fieldError }}</p>
    <p v-if="serverError" role="alert" class="project-form__alert">{{ serverError }}</p>
    <div class="project-form__actions">
      <UButton type="submit" :disabled="busy">
        {{ busy ? t("project.clone.pending") : t("project.clone") }}
      </UButton>
      <UButton type="button" variant="outline" color="neutral" @click="onCancel">
        {{ t("common.cancel") }}
      </UButton>
    </div>
  </form>
</template>
