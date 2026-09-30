<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { ref } from "vue";
import { problemMessage } from "@/lib/api";
import type { SetupInput } from "@/lib/contracts";
import { setupInput } from "@/lib/validators";
import AuthAlert from "./AuthAlert.vue";
import AuthField from "./AuthField.vue";
import AuthLayout from "./AuthLayout.vue";
import AuthPanel from "./AuthPanel.vue";
import { useAuthForm } from "./useAuthForm";

const props = defineProps<{
  submitSetup: (input: SetupInput) => Promise<void>;
  brandingName?: string | null;
}>();

const serverError = ref<string | null>(null);
const form = useAuthForm({
  initial: {
    familyName: "",
    givenName: "",
    email: "",
    password: "",
    workspaceName: "",
    workspaceSlug: "",
  },
  schema: setupInput,
  ids: {
    familyName: "setup-family-name",
    givenName: "setup-given-name",
    email: "setup-email",
    password: "setup-password",
    workspaceName: "setup-workspace-name",
    workspaceSlug: "setup-workspace-slug",
  },
});

const submit = form.handleSubmit(async (values) => {
  serverError.value = null;
  try {
    await props.submitSetup(values);
  } catch (err) {
    serverError.value = problemMessage(err, "error.auth.setup");
  }
});
</script>

<template>
  <AuthLayout :branding-name="brandingName">
    <AuthPanel :title="t('auth.setup.title')">
      <form class="auth-shell__stack auth-shell__stack--form" novalidate @submit="submit">
        <div class="grid grid-cols-[6rem_minmax(0,1fr)] gap-3">
          <AuthField
            id="setup-family-name"
            name="familyName"
            autocomplete="family-name"
            :label="t('settings.familyName')"
            :error="form.errors.familyName"
            @input="form.onInput('familyName', $event)"
          />
          <AuthField
            id="setup-given-name"
            name="givenName"
            autocomplete="given-name"
            :label="t('settings.givenName')"
            :error="form.errors.givenName"
            @input="form.onInput('givenName', $event)"
          />
        </div>
        <AuthField
          id="setup-email"
          name="email"
          type="email"
          autocomplete="email"
          :label="t('auth.email')"
          :error="form.errors.email"
          @input="form.onInput('email', $event)"
        />
        <AuthField
          id="setup-password"
          name="password"
          type="password"
          autocomplete="new-password"
          :label="t('auth.password')"
          :error="form.errors.password"
          @input="form.onInput('password', $event)"
        />
        <AuthField
          id="setup-workspace-name"
          name="workspaceName"
          :label="t('workspace.name')"
          :error="form.errors.workspaceName"
          @input="form.onInput('workspaceName', $event)"
        />
        <AuthField
          id="setup-workspace-slug"
          name="workspaceSlug"
          placeholder="my-workspace"
          autocomplete="off"
          :spellcheck="false"
          :label="t('auth.setup.slug')"
          :hint="t('form.pattern.slug')"
          :error="form.errors.workspaceSlug"
          @input="form.onInput('workspaceSlug', $event)"
        />
        <AuthAlert v-if="serverError" :message="serverError" />
        <UButton
          type="submit"
          size="lg"
          class="auth-shell__button"
          :disabled="form.submitting.value"
        >
          {{ form.submitting.value ? t("form.creating") : t("auth.setup.start") }}
        </UButton>
      </form>
    </AuthPanel>
  </AuthLayout>
</template>
