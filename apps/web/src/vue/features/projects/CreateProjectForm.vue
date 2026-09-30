<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { computed, ref } from "vue";
import { ProblemError } from "@/lib/api";
import { canonicalizeProjectKey } from "@/lib/href";
import type { ProjectCreateBody } from "@/features/projects/create-payload";
import type { components } from "@/generated/api";
import LeadSelect from "./LeadSelect.vue";
import { projectFormPayload } from "./project-form";

type Member = components["schemas"]["MemberResponse"];

const props = defineProps<{
  pending?: boolean;
  members: readonly Member[];
  currentUserId: string | null;
  onSubmit: (input: ProjectCreateBody) => Promise<void>;
  onCancel: () => void;
}>();

const key = ref("");
const name = ref("");
const visibility = ref<"workspace" | "private">("workspace");
const description = ref("");
const icon = ref("");
const leadUserId = ref(props.currentUserId ?? "");
const fieldError = ref<string | null>(null);
const serverError = ref<string | null>(null);
const submitting = ref(false);
const busy = computed(() => props.pending === true || submitting.value);

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
  submitting.value = true;
  try {
    await props.onSubmit(parsed.body);
  } catch (err) {
    serverError.value = err instanceof ProblemError ? err.title : t("error.network");
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <form class="project-form" novalidate @submit="submitForm">
    <div class="project-form__row">
      <div class="project-form__field">
        <label for="project-key">{{ t("project.keyLabel") }}</label>
        <input
          id="project-key"
          :value="key"
          placeholder="LAB"
          autocomplete="off"
          :disabled="busy"
          @input="onKeyInput"
        />
        <p class="project-form__hint">{{ t("form.pattern.key") }}</p>
      </div>
      <div class="project-form__field">
        <label for="project-name">{{ t("project.name") }}</label>
        <input id="project-name" v-model="name" :disabled="busy" />
      </div>
      <div class="project-form__field">
        <label for="project-icon">{{ t("project.icon") }}</label>
        <input id="project-icon" v-model="icon" :disabled="busy" />
      </div>
      <div class="project-form__field">
        <label for="project-visibility">{{ t("project.visibility") }}</label>
        <select id="project-visibility" v-model="visibility" :disabled="busy">
          <option value="workspace">{{ t("project.visibility.workspaceAll") }}</option>
          <option value="private">{{ t("project.visibility.private") }}</option>
        </select>
      </div>
      <LeadSelect id="project-lead" v-model="leadUserId" :members="members" :disabled="busy" />
    </div>
    <div class="project-form__field">
      <label for="project-description">{{ t("project.description") }}</label>
      <textarea
        id="project-description"
        v-model="description"
        rows="2"
        :placeholder="t('project.description.placeholder')"
        :disabled="busy"
      />
    </div>
    <p v-if="fieldError" role="alert" class="project-form__alert">{{ fieldError }}</p>
    <p v-if="serverError" role="alert" class="project-form__alert">{{ serverError }}</p>
    <div class="project-form__actions">
      <UButton type="submit" :disabled="busy">
        {{ busy ? t("project.create.pending") : t("project.new") }}
      </UButton>
      <UButton type="button" variant="outline" color="neutral" @click="onCancel">
        {{ t("common.cancel") }}
      </UButton>
    </div>
  </form>
</template>
