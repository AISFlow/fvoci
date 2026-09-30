<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { ref } from "vue";
import { startWorkspaceSso, type WorkspaceSsoSlugIssue } from "@/lib/oidc";
import AuthField from "./AuthField.vue";

const issue = ref<WorkspaceSsoSlugIssue | null>(null);

function onSubmit(event: Event): void {
  event.preventDefault();
  const form = event.target;
  if (!(form instanceof HTMLFormElement)) return;
  const slug = new FormData(form).get("slug");
  issue.value = startWorkspaceSso(typeof slug === "string" ? slug : "");
}
</script>

<template>
  <!-- Not a form submission to the server: startWorkspaceSso navigates by
       script, since the server's redirect to the IdP would break form-action. -->
  <form class="auth-shell__stack" novalidate @submit="onSubmit">
    <AuthField
      id="login-sso-slug"
      name="slug"
      autocomplete="off"
      maxlength="32"
      :label="t('auth.sso.slug')"
      :error="issue ? t(issue) : undefined"
    />
    <UButton type="submit" variant="outline" size="lg" color="neutral" class="auth-shell__button">
      {{ t("auth.sso.login") }}
    </UButton>
  </form>
</template>
