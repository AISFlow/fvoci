<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import UInput from "@nuxt/ui/components/Input.vue";
import { computed, ref } from "vue";
import { clickOidcStart, oidcErrorMessage, startOidcLink } from "@/lib/oidc";
import { ProblemError, problemMessage } from "@/lib/api";
import type {
  IdentityOutput,
  PasswordChangeInput,
  ProfileNameInput,
  ProviderOutput,
  SessionUserOutput,
  WithdrawInput,
} from "@/lib/contracts";
import type { components } from "@/generated/api";
import {
  emailChangeInput,
  passwordChangeForm,
  passwordCreateForm,
  profileNameInput,
  withdrawConfirmForm,
} from "@/lib/validators";
import ConfirmAction from "./ConfirmAction.vue";
import { fieldIssue, parseForm } from "./form";
import MfaSection from "./MfaSection.vue";
import "@/features/settings/settings-shell.css";

type MfaSetupOutput = components["schemas"]["MfaSetupOutput"];
type MfaStatusOutput = components["schemas"]["MfaStatusOutput"];

const props = defineProps<{
  me: SessionUserOutput;
  identities: IdentityOutput[];
  providers: ProviderOutput[];
  magicLink: boolean;
  successNotice?: string | null;
  errorNotice?: string | null;
  mfaStatus: MfaStatusOutput;
  onSaveName: (input: ProfileNameInput) => Promise<void>;
  onSendVerification: (email: string) => Promise<void>;
  onChangeEmail: (newEmail: string) => Promise<void>;
  onChangePassword: (input: PasswordChangeInput) => Promise<void>;
  onWithdraw: (input: WithdrawInput) => Promise<void>;
  onExport: () => Promise<void>;
  onUnlink: (provider: string) => Promise<void>;
  onMfaSetup: (input: components["schemas"]["MfaSetupBody"]) => Promise<MfaSetupOutput>;
  onMfaEnable: (code: string) => Promise<void>;
  onMfaDisable: (input: components["schemas"]["MfaDisableBody"]) => Promise<void>;
}>();

const verifyPending = ref(false);
const verifySent = ref(false);
const verifyError = ref<string | null>(null);
const newEmail = ref("");
const emailFieldError = ref<string | null>(null);
const emailServerError = ref<string | null>(null);
const emailSent = ref(false);
const emailBusy = ref(false);
const familyName = ref(props.me.familyName ?? "");
const givenName = ref(props.me.givenName);
const nameFieldError = ref<string | null>(null);
const nameSaveError = ref<string | null>(null);
const nameBusy = ref(false);
const currentPassword = ref("");
const newPassword = ref("");
const currentPasswordError = ref<string | null>(null);
const newPasswordError = ref<string | null>(null);
const passwordServerError = ref<string | null>(null);
const passwordSuccess = ref(false);
const passwordBusy = ref(false);
const unlinkPending = ref<string | null>(null);
const methodError = ref<string | null>(null);
const linkPending = ref<string | null>(null);
const exportPending = ref(false);
const exportError = ref<string | null>(null);
const withdrawValue = ref("");
const withdrawFieldError = ref<string | null>(null);
const withdrawError = ref<string | null>(null);
const withdrawBusy = ref(false);

const linkedByProvider = computed(() => new Map(props.identities.map((item) => [item.provider, item])));

async function sendVerification(): Promise<void> {
  verifyError.value = null;
  verifyPending.value = true;
  try {
    await props.onSendVerification(props.me.email);
    verifySent.value = true;
  } catch (err) {
    verifyError.value = problemMessage(err, "error.email.verifySend");
  } finally {
    verifyPending.value = false;
  }
}

async function changeEmail(): Promise<void> {
  emailFieldError.value = null;
  emailServerError.value = null;
  const parsed = parseForm(emailChangeInput, { newEmail: newEmail.value });
  if (!parsed.ok) {
    emailFieldError.value = parsed.message;
    return;
  }
  emailBusy.value = true;
  try {
    await props.onChangeEmail(parsed.data.newEmail);
    emailSent.value = true;
    newEmail.value = "";
  } catch (err) {
    emailServerError.value = problemMessage(err, "error.email.change");
  } finally {
    emailBusy.value = false;
  }
}

async function saveName(): Promise<void> {
  nameFieldError.value = null;
  nameSaveError.value = null;
  const parsed = parseForm(profileNameInput, { familyName: familyName.value, givenName: givenName.value });
  if (!parsed.ok) {
    nameFieldError.value = parsed.message;
    return;
  }
  nameBusy.value = true;
  try {
    await props.onSaveName({
      givenName: parsed.data.givenName,
      familyName: parsed.data.familyName === "" ? null : parsed.data.familyName,
    });
  } catch (err) {
    nameSaveError.value = problemMessage(err, "settings.save.failed");
  } finally {
    nameBusy.value = false;
  }
}

async function changePassword(): Promise<void> {
  currentPasswordError.value = null;
  newPasswordError.value = null;
  passwordServerError.value = null;
  passwordSuccess.value = false;
  const schema = props.me.hasPassword ? passwordChangeForm : passwordCreateForm;
  const body = { currentPassword: currentPassword.value, newPassword: newPassword.value };
  const parsed = parseForm(schema, body);
  if (!parsed.ok) {
    currentPasswordError.value = props.me.hasPassword ? fieldIssue(schema, body, "currentPassword") : null;
    newPasswordError.value = fieldIssue(schema, body, "newPassword");
    return;
  }
  passwordBusy.value = true;
  try {
    await props.onChangePassword({
      currentPassword: props.me.hasPassword ? parsed.data.currentPassword : null,
      newPassword: parsed.data.newPassword,
    });
    passwordSuccess.value = true;
    currentPassword.value = "";
    newPassword.value = "";
  } catch (err) {
    passwordServerError.value = problemMessage(err, "error.password.change");
  } finally {
    passwordBusy.value = false;
  }
}

async function handleUnlink(provider: string): Promise<void> {
  methodError.value = null;
  unlinkPending.value = provider;
  try {
    await props.onUnlink(provider);
  } catch (err) {
    methodError.value =
      err instanceof ProblemError
        ? err.code === "oidc_last_method"
          ? oidcErrorMessage(err.code)
          : err.titleKnown
            ? err.title
            : t("error.unlink")
        : t("error.network");
  } finally {
    unlinkPending.value = null;
  }
}

async function handleExport(): Promise<void> {
  exportError.value = null;
  exportPending.value = true;
  try {
    await props.onExport();
  } catch (err) {
    exportError.value = problemMessage(err, "error.export.me");
  } finally {
    exportPending.value = false;
  }
}

async function withdraw(): Promise<void> {
  withdrawFieldError.value = null;
  withdrawError.value = null;
  const parsed = parseForm(withdrawConfirmForm, { confirmValue: withdrawValue.value });
  if (!parsed.ok) {
    withdrawFieldError.value = parsed.message;
    return;
  }
  withdrawBusy.value = true;
  try {
    await props.onWithdraw({
      currentPassword: props.me.hasPassword ? parsed.data.confirmValue : null,
      emailLocalPart: props.me.hasPassword ? null : parsed.data.confirmValue,
    });
  } catch (err) {
    withdrawError.value = problemMessage(err, "error.withdraw");
  } finally {
    withdrawBusy.value = false;
  }
}
</script>

<template>
  <div class="settings-stack">
    <section class="settings-section">
      <h1 class="settings-section__title">{{ t("auth.account.title") }}</h1>
      <div class="flex flex-col gap-6">
        <p v-if="successNotice" role="status" class="text-sm text-muted">{{ successNotice }}</p>
        <p v-if="errorNotice" role="alert" class="text-sm text-error">{{ errorNotice }}</p>
        <div class="flex flex-col gap-1.5">
          <div class="flex items-center gap-2">
            <p class="text-sm text-muted" data-testid="account-email">{{ me.email }}</p>
            <span role="status" class="text-xs text-muted">
              {{ me.emailVerifiedAt !== null ? t("auth.account.email.verified") : t("auth.account.email.unverified") }}
            </span>
          </div>
          <div v-if="me.emailVerifiedAt === null" class="flex flex-col gap-1.5">
            <UButton
              type="button"
              variant="outline"
              color="neutral"
              size="sm"
              class="w-fit"
              :disabled="!magicLink || verifyPending"
              @click="sendVerification"
            >
              {{ t("auth.account.email.sendVerify") }}
            </UButton>
            <p v-if="!magicLink" class="break-keep text-sm text-muted">{{ t("auth.account.email.sendDisabled") }}</p>
            <p v-if="verifyError" role="alert" class="text-sm text-error">{{ verifyError }}</p>
            <p v-if="verifySent" role="status" class="text-sm text-muted">{{ t("auth.account.email.verifySent") }}</p>
          </div>
        </div>
        <form class="flex flex-col gap-1.5" novalidate @submit.prevent="changeEmail">
          <label for="settings-new-email">{{ t("auth.account.email.new") }}</label>
          <div class="flex gap-2">
            <UInput
              id="settings-new-email"
              v-model="newEmail"
              type="email"
              autocomplete="email"
              :disabled="!magicLink"
              :aria-invalid="emailFieldError ? true : undefined"
            />
            <UButton type="submit" size="sm" :disabled="!magicLink || emailBusy">
              {{ emailBusy ? t("auth.emailChange.requesting") : t("common.change") }}
            </UButton>
          </div>
          <p v-if="!magicLink" class="break-keep text-sm text-muted">{{ t("auth.account.email.sendDisabled") }}</p>
          <p v-if="emailFieldError" role="alert" class="text-sm text-error">{{ emailFieldError }}</p>
          <p v-if="emailServerError" role="alert" class="text-sm text-error">{{ emailServerError }}</p>
          <p v-if="emailSent" role="status" class="break-keep text-sm text-muted">{{ t("auth.account.email.changeSent") }}</p>
        </form>
        <form class="flex flex-col gap-1.5" novalidate @submit.prevent="saveName">
          <label for="settings-given-name">{{ t("settings.givenName") }}</label>
          <div class="flex gap-2">
            <UInput
              id="settings-family-name"
              v-model="familyName"
              class="w-24 flex-none"
              :aria-label="t('settings.familyName')"
              autocomplete="family-name"
            />
            <UInput id="settings-given-name" v-model="givenName" class="min-w-0 flex-1" autocomplete="given-name" />
            <UButton type="submit" size="sm" :disabled="nameBusy">
              {{ nameBusy ? t("settings.profile.saving") : t("settings.profile.save") }}
            </UButton>
          </div>
          <p v-if="nameFieldError" role="alert" class="text-sm text-error">{{ nameFieldError }}</p>
          <p v-if="nameSaveError" role="alert" class="text-sm text-error">{{ nameSaveError }}</p>
        </form>
        <div class="flex flex-col gap-2">
          <p class="text-sm font-medium">{{ t("auth.password") }}</p>
          <form class="flex flex-col gap-1.5" novalidate @submit.prevent="changePassword">
            <template v-if="me.hasPassword">
              <label for="settings-current-password">{{ t("auth.passwordCurrent") }}</label>
              <UInput
                id="settings-current-password"
                v-model="currentPassword"
                type="password"
                autocomplete="current-password"
                :aria-invalid="currentPasswordError ? true : undefined"
                :aria-describedby="currentPasswordError ? 'settings-current-password-error' : undefined"
              />
              <p
                v-if="currentPasswordError"
                id="settings-current-password-error"
                role="alert"
                class="text-sm text-error"
              >
                {{ currentPasswordError }}
              </p>
            </template>
            <label for="settings-new-password">{{ me.hasPassword ? t("auth.passwordNew") : t("auth.password") }}</label>
            <UInput
              id="settings-new-password"
              v-model="newPassword"
              type="password"
              autocomplete="new-password"
              :aria-invalid="newPasswordError ? true : undefined"
              :aria-describedby="newPasswordError ? 'settings-new-password-error' : undefined"
            />
            <p v-if="newPasswordError" id="settings-new-password-error" role="alert" class="text-sm text-error">
              {{ newPasswordError }}
            </p>
            <p v-if="passwordServerError" role="alert" class="text-sm text-error">{{ passwordServerError }}</p>
            <p v-if="passwordSuccess" role="status" class="text-sm text-muted">{{ t("auth.account.password.changed") }}</p>
            <UButton type="submit" size="sm" :disabled="passwordBusy">
              {{
                passwordBusy
                  ? t("form.changing")
                  : me.hasPassword
                    ? t("auth.reset.change")
                    : t("auth.account.password.create")
              }}
            </UButton>
          </form>
        </div>
        <MfaSection
          :status="mfaStatus"
          :has-password="me.hasPassword"
          :on-setup="onMfaSetup"
          :on-enable="onMfaEnable"
          :on-disable="onMfaDisable"
        />
        <div class="flex flex-col gap-2">
          <p class="text-sm font-medium">{{ t("auth.account.social.title") }}</p>
          <p v-if="providers.length === 0" class="text-sm text-muted">{{ t("auth.account.social.empty") }}</p>
          <div v-for="item in providers" :key="item.provider" class="flex items-center justify-between gap-2 text-sm">
            <span>
              {{ item.label }}
              <span v-if="linkedByProvider.get(item.provider)" class="text-muted">
                — {{ linkedByProvider.get(item.provider)?.email ?? t("common.connected") }}
              </span>
            </span>
            <ConfirmAction
              v-if="linkedByProvider.get(item.provider)"
              :title="t('auth.account.unlink.confirm.title')"
              :description="t('auth.account.unlink.confirm.body', { label: item.label })"
              :action-label="t('common.unlink')"
              :disabled="unlinkPending === item.provider"
              :run="() => handleUnlink(item.provider)"
            >
              {{ t("common.unlink") }}
            </ConfirmAction>
            <UButton
              v-else
              type="button"
              variant="outline"
              color="neutral"
              size="sm"
              :disabled="linkPending !== null"
              :aria-busy="linkPending === item.provider ? true : undefined"
              @click="
                clickOidcStart(
                  item.provider,
                  () => startOidcLink(item.provider),
                  { setPending: (value) => (linkPending = value), setError: (value) => (methodError = value) },
                  'error.link',
                )
              "
            >
              {{ t("common.link") }}
            </UButton>
          </div>
          <p v-if="methodError" role="alert" class="text-sm text-error">{{ methodError }}</p>
        </div>
      </div>
    </section>
    <section class="settings-section">
      <UButton
        type="button"
        variant="outline"
        color="neutral"
        size="sm"
        class="w-fit"
        :disabled="exportPending"
        @click="handleExport"
      >
        {{ t("export.me") }}
      </UButton>
      <p v-if="exportError" role="alert" class="settings-notice settings-notice--danger">{{ exportError }}</p>
    </section>
    <section class="settings-section">
      <h2 class="settings-section__title">{{ t("auth.account.withdraw.title") }}</h2>
      <form class="flex flex-col gap-1.5" novalidate @submit.prevent="withdraw">
        <label for="withdraw-confirm">{{
          me.hasPassword ? t("auth.passwordCurrent") : t("auth.account.withdraw.localPart")
        }}</label>
        <p class="break-keep text-sm text-muted">{{ t("auth.account.withdraw.body") }}</p>
        <p v-if="!me.hasPassword" class="break-keep text-sm text-muted">{{ t("auth.account.withdraw.hint") }}</p>
        <UInput
          id="withdraw-confirm"
          v-model="withdrawValue"
          :type="me.hasPassword ? 'password' : 'text'"
          :autocomplete="me.hasPassword ? 'current-password' : 'off'"
        />
        <p v-if="withdrawFieldError || withdrawError" role="alert" class="text-sm text-error">
          {{ withdrawFieldError ?? withdrawError }}
        </p>
        <UButton type="submit" color="error" size="sm" :disabled="withdrawBusy">
          {{ withdrawBusy ? t("auth.withdraw.pending") : t("auth.account.withdraw.submit") }}
        </UButton>
      </form>
    </section>
  </div>
</template>
