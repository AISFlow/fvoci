<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { computed, reactive, ref } from "vue";
import { problemMessage } from "@/lib/api";
import type {
  InvitationAcceptInput,
  InvitationPublicOutput,
  ProviderOutput,
} from "@/lib/contracts";
import { clickOidcStart, startOidcInvite } from "@/lib/oidc";
import { invitationAcceptInput } from "@/lib/validators";
import AuthAlert from "./AuthAlert.vue";
import AuthField from "./AuthField.vue";
import AuthLayout from "./AuthLayout.vue";
import AuthPanel from "./AuthPanel.vue";
import AuthStatus from "./AuthStatus.vue";
import { useAuthForm } from "./useAuthForm";

function roleLabel(role: string): string {
  if (role === "owner") return t("role.owner");
  if (role === "admin") return t("role.admin");
  if (role === "guest") return t("role.guest");
  return t("role.member");
}

function consentKey(doc: { kind: string; version: number }): string {
  return `${doc.kind}:${doc.version}`;
}

const props = defineProps<{
  invitation: InvitationPublicOutput;
  token: string;
  brandingName?: string | null;
  submitAccept: (input: InvitationAcceptInput) => Promise<void>;
  providers?: ProviderOutput[];
}>();

const serverError = ref<string | null>(null);
const startingProvider = ref<string | null>(null);
const consentChecked = reactive<Record<string, boolean>>({});
const consentItems = computed(() =>
  props.invitation.requiredLegal.map((doc) => ({ kind: doc.kind, version: doc.version })),
);
const allConsented = computed(() =>
  props.invitation.requiredLegal.every((doc) => consentChecked[consentKey(doc)] === true),
);

const form = useAuthForm({
  initial: { email: "", familyName: "", givenName: "", password: "" },
  schema: invitationAcceptInput,
  ids: {
    email: "invite-email",
    familyName: "invite-family-name",
    givenName: "invite-given-name",
    password: "invite-password",
  },
});

const onSubmit = form.handleSubmit(async (values) => {
  serverError.value = null;
  try {
    await props.submitAccept({
      ...values,
      ...(consentItems.value.length > 0 ? { consents: consentItems.value } : {}),
    });
  } catch (err) {
    serverError.value = problemMessage(err, "error.auth.invite");
  }
});

function startProvider(provider: string): void {
  void clickOidcStart(
    provider,
    () => startOidcInvite(provider, { token: props.token, consents: consentItems.value }),
    {
      setPending: (next) => {
        startingProvider.value = next;
      },
      setError: (message) => {
        serverError.value = message;
      },
    },
    "error.auth.invite",
  );
}
</script>

<template>
  <AuthLayout :branding-name="brandingName">
    <AuthPanel
      :title="t('auth.invite.title', { name: invitation.workspaceName })"
      :lead="
        t('auth.invite.body', {
          email: invitation.emailMasked,
          role: roleLabel(invitation.role),
        })
      "
    >
      <div
        v-if="invitation.requiredLegal.length > 0"
        class="auth-shell__stack auth-shell__divided pb-5"
      >
        <p class="auth-shell__text auth-shell__text--strong">{{ t("auth.invite.consents") }}</p>
        <div
          v-for="doc in invitation.requiredLegal"
          :key="consentKey(doc)"
          class="auth-shell__check"
        >
          <input
            :id="`invite-consent-${consentKey(doc)}`"
            type="checkbox"
            :checked="consentChecked[consentKey(doc)] ?? false"
            @change="consentChecked[consentKey(doc)] = ($event.target as HTMLInputElement).checked"
          />
          <div class="flex min-w-0 flex-col gap-1">
            <label :for="`invite-consent-${consentKey(doc)}`" class="auth-shell__text">{{
              doc.title
            }}</label>
            <a
              :href="`/legal/${doc.kind}`"
              target="_blank"
              rel="noreferrer"
              class="auth-shell__link auth-shell__dense w-fit"
            >
              {{ t("common.view") }}
            </a>
          </div>
        </div>
      </div>
      <form class="auth-shell__stack auth-shell__stack--form" novalidate @submit="onSubmit">
        <AuthField
          id="invite-email"
          name="email"
          type="email"
          autocomplete="email"
          :label="t('auth.email')"
          :error="form.errors.email"
          :disabled="form.submitting.value"
          @input="form.onInput('email', $event)"
        />
        <div class="auth-shell__name-row">
          <AuthField
            id="invite-family-name"
            name="familyName"
            autocomplete="family-name"
            :label="t('settings.familyName')"
            :error="form.errors.familyName"
            :disabled="form.submitting.value"
            @input="form.onInput('familyName', $event)"
          />
          <AuthField
            id="invite-given-name"
            name="givenName"
            autocomplete="given-name"
            :label="t('settings.givenName')"
            :error="form.errors.givenName"
            :disabled="form.submitting.value"
            @input="form.onInput('givenName', $event)"
          />
        </div>
        <AuthStatus :message="t('auth.invite.newAccountHint')" />
        <AuthField
          id="invite-password"
          name="password"
          type="password"
          autocomplete="new-password"
          :label="t('auth.password')"
          :error="form.errors.password"
          :disabled="form.submitting.value"
          @input="form.onInput('password', $event)"
        />
        <AuthAlert v-if="serverError" :message="serverError" />
        <UButton
          type="submit"
          size="lg"
          class="auth-shell__button"
          :disabled="!allConsented || form.submitting.value"
        >
          {{ form.submitting.value ? t("auth.invite.accepting") : t("auth.invite.accept") }}
        </UButton>
      </form>
      <template v-if="providers && providers.length > 0">
        <hr class="auth-shell__rule" />
        <p class="auth-shell__text auth-shell__text--strong auth-shell__text--muted">{{
          t("auth.invite.social")
        }}</p>
        <div class="auth-shell__stack">
          <UButton
            v-for="p in providers"
            :key="p.provider"
            type="button"
            variant="outline"
            size="lg"
            color="neutral"
            class="auth-shell__button"
            :disabled="!allConsented || startingProvider !== null"
            :aria-busy="startingProvider === p.provider ? true : undefined"
            @click="startProvider(p.provider)"
          >
            {{ p.label }}
          </UButton>
        </div>
      </template>
    </AuthPanel>
  </AuthLayout>
</template>
