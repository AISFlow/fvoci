<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UPageCard from "@nuxt/ui/components/PageCard.vue";
import UFormField from "@nuxt/ui/components/FormField.vue";
import USelect from "@nuxt/ui/components/Select.vue";
import UButton from "@nuxt/ui/components/Button.vue";
import { computed, ref } from "vue";
import { z } from "zod";
import ConfirmAction from "../../components/ConfirmAction.vue";
import { inputText, useZodForm } from "../../composables/useZodForm";
import { ProblemError, problemMessage } from "@/lib/api";
import type {
  IdentityOutput,
  PasswordChangeInput,
  ProfileNameInput,
  ProviderOutput,
  SessionUserOutput,
  WithdrawInput,
} from "@/lib/contracts";
import { clickOidcStart, oidcErrorMessage, startOidcLink } from "@/lib/oidc";
import {
  emailChangeInput,
  passwordChangeForm,
  passwordCreateForm,
  profileNameInput,
  withdrawConfirmForm,
} from "@/lib/validators";
import { fieldClass } from "./field-classes";
import { settingLabel } from "@/features/settings/settings-instance-model";
import {
  readThemePreference,
  setThemePreference,
  type ThemePreference,
} from "@/lib/ui-preferences";
import { optionKey, SETTING_ENUM_OPTIONS } from "@/features/settings/settings-catalog";
import "@/features/settings/settings-shell.css";

const props = defineProps<{
  me: SessionUserOutput;
  identities: IdentityOutput[];
  providers: ProviderOutput[];
  magicLink: boolean;
  successNotice?: string | null;
  errorNotice?: string | null;
  onSaveName: (input: ProfileNameInput) => Promise<void>;
  onSavePreferences: (input: {
    locale: "ko";
    timezone: string;
    weekStartsOn: number;
    textScale: number;
  }) => Promise<void>;
  onSendVerification: (email: string) => Promise<void>;
  onChangeEmail: (newEmail: string) => Promise<void>;
  onChangePassword: (input: PasswordChangeInput) => Promise<void>;
  onWithdraw: (input: WithdrawInput) => Promise<void>;
  onExport: () => Promise<void>;
  onUnlink: (provider: string) => Promise<void>;
}>();

const SEND_DISABLED_NOTICE = t("auth.account.email.sendDisabled");
const VERIFY_SENT_NOTICE = t("auth.account.email.verifySent");
const EMAIL_CHANGE_SENT_NOTICE = t("auth.account.email.changeSent");
const PASSWORD_CHANGED_NOTICE = t("auth.account.password.changed");
const WITHDRAW_LOCAL_PART_HINT = t("auth.account.withdraw.hint");

const verifyPending = ref(false);
const verifySent = ref(false);
const verifyError = ref<string | null>(null);

async function sendVerify(): Promise<void> {
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

const emailForm = useZodForm({
  schema: () => emailChangeInput,
  defaults: () => ({ newEmail: "" }),
  fieldIds: { newEmail: "settings-new-email" },
});
const emailServerError = ref<string | null>(null);
const emailSent = ref(false);

async function submitEmail(): Promise<void> {
  await emailForm.submit(async (data) => {
    emailServerError.value = null;
    try {
      await props.onChangeEmail(data.newEmail);
      emailSent.value = true;
      emailForm.reset();
    } catch (err) {
      emailServerError.value = problemMessage(err, "error.email.change");
    }
  });
}

const nameForm = useZodForm({
  schema: () => profileNameInput,
  defaults: () => ({ familyName: props.me.familyName ?? "", givenName: props.me.givenName }),
  fieldIds: { familyName: "settings-family-name", givenName: "settings-given-name" },
});
const nameError = ref<string | null>(null);
const nameFieldError = () =>
  nameForm.errors.value.familyName ?? nameForm.errors.value.givenName ?? null;

async function submitName(): Promise<void> {
  await nameForm.submit(async (data) => {
    nameError.value = null;
    try {
      await props.onSaveName({
        givenName: data.givenName,
        familyName: data.familyName === "" ? null : data.familyName,
      });
    } catch (err) {
      nameError.value = problemMessage(err, "settings.save.failed");
    }
  });
}

const preferenceForm = useZodForm({
  schema: () =>
    z.object({
      locale: z.literal("ko"),
      timezone: z.string().min(1, "i18n:form.too_small"),
      weekStartsOn: z.enum(["0", "1"]).transform(Number),
      textScale: z.enum(["16", "18", "20"]).transform(Number),
    }),
  defaults: () => ({
    locale: "ko" as const,
    timezone: props.me.timezone,
    weekStartsOn: String(props.me.weekStartsOn),
    textScale: String(props.me.textScale),
  }),
  fieldIds: {
    locale: "settings-locale",
    timezone: "settings-timezone",
    weekStartsOn: "settings-week-start",
    textScale: "settings-text-scale",
  },
});
const timezones = computed(() => [
  ...new Set([props.me.timezone, "UTC", ...Intl.supportedValuesOf("timeZone")]),
]);
const preferenceError = ref<string | null>(null);
const preferencesSaved = ref(false);
const theme = ref<ThemePreference>(readThemePreference());

function changeTheme(value: unknown): void {
  if (value !== "system" && value !== "light" && value !== "dark") return;
  theme.value = value;
  setThemePreference(value);
}

async function submitPreferences(): Promise<void> {
  await preferenceForm.submit(async (input) => {
    preferenceError.value = null;
    preferencesSaved.value = false;
    try {
      await props.onSavePreferences(input);
      preferencesSaved.value = true;
    } catch (err) {
      preferenceError.value = problemMessage(err, "settings.save.failed");
    }
  });
}

const passwordForm = useZodForm({
  schema: () => (props.me.hasPassword ? passwordChangeForm : passwordCreateForm),
  defaults: () => ({ currentPassword: "", newPassword: "" }),
  fieldIds: { currentPassword: "settings-current-password", newPassword: "settings-new-password" },
});
const passwordServerError = ref<string | null>(null);
const passwordSuccess = ref(false);

async function submitPassword(): Promise<void> {
  await passwordForm.submit(async (data) => {
    passwordServerError.value = null;
    passwordSuccess.value = false;
    try {
      await props.onChangePassword({
        currentPassword: props.me.hasPassword ? data.currentPassword : null,
        newPassword: data.newPassword,
      });
      passwordSuccess.value = true;
      passwordForm.reset();
    } catch (err) {
      passwordServerError.value = problemMessage(err, "error.password.change");
    }
  });
}

const unlinkPending = ref<string | null>(null);
const linkPending = ref<string | null>(null);
const methodError = ref<string | null>(null);
const linkedByProvider = () => new Map(props.identities.map((i) => [i.provider, i]));

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

async function linkProvider(provider: string): Promise<void> {
  await clickOidcStart(
    provider,
    () => startOidcLink(provider),
    {
      setPending: (next) => {
        linkPending.value = next;
      },
      setError: (message) => {
        methodError.value = message;
      },
    },
    "error.link",
  );
}

const withdrawForm = useZodForm({
  schema: () => withdrawConfirmForm,
  defaults: () => ({ confirmValue: "" }),
  fieldIds: { confirmValue: "withdraw-confirm" },
});
const withdrawError = ref<string | null>(null);

async function submitWithdraw(): Promise<void> {
  await withdrawForm.submit(async (data) => {
    withdrawError.value = null;
    try {
      await props.onWithdraw({
        currentPassword: props.me.hasPassword ? data.confirmValue : null,
        emailLocalPart: props.me.hasPassword ? null : data.confirmValue,
      });
    } catch (err) {
      withdrawError.value = problemMessage(err, "error.withdraw");
    }
  });
}

const exportPending = ref(false);
const exportError = ref<string | null>(null);

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
</script>

<template>
  <div class="settings-stack">
    <UPageCard
      as="section"
      variant="subtle"
      class="settings-section"
      aria-labelledby="account-title"
    >
      <h1 id="account-title" class="settings-section__title text-title">{{
        t("auth.account.title")
      }}</h1>
      <div class="flex flex-col gap-6">
        <p v-if="successNotice" role="status" class="text-sm text-muted">{{ successNotice }}</p>
        <p v-if="errorNotice" role="alert" class="text-sm text-error">{{ errorNotice }}</p>

        <div class="flex flex-col gap-1.5">
          <div class="flex items-center gap-2">
            <p class="text-sm text-muted" data-testid="account-email">{{ me.email }}</p>
            <span role="status" class="text-xs text-muted">
              {{
                me.emailVerifiedAt !== null
                  ? t("auth.account.email.verified")
                  : t("auth.account.email.unverified")
              }}
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
              @click="sendVerify"
            >
              {{ t("auth.account.email.sendVerify") }}
            </UButton>
            <p v-if="!magicLink" class="break-keep text-sm text-muted">{{
              SEND_DISABLED_NOTICE
            }}</p>
            <p v-if="verifyError" role="alert" class="text-sm text-error">{{ verifyError }}</p>
            <p v-if="verifySent" role="status" class="text-sm text-muted">{{
              VERIFY_SENT_NOTICE
            }}</p>
          </div>
        </div>

        <form class="flex flex-col gap-1.5" novalidate @submit.prevent="submitEmail">
          <label class="text-sm font-medium" for="settings-new-email">{{
            t("auth.account.email.new")
          }}</label>
          <div class="flex gap-2">
            <input
              id="settings-new-email"
              :class="['min-w-0 flex-1', fieldClass]"
              type="email"
              autocomplete="email"
              :disabled="!magicLink"
              :aria-invalid="emailForm.errors.value.newEmail ? true : undefined"
              :value="emailForm.values.newEmail"
              @input="emailForm.values.newEmail = inputText($event)"
            />
            <UButton type="submit" size="sm" :disabled="!magicLink || emailForm.submitting.value">
              {{
                emailForm.submitting.value ? t("auth.emailChange.requesting") : t("common.change")
              }}
            </UButton>
          </div>
          <p v-if="!magicLink" class="break-keep text-sm text-muted">{{ SEND_DISABLED_NOTICE }}</p>
          <p v-if="emailForm.errors.value.newEmail" role="alert" class="text-sm text-error">
            {{ emailForm.errors.value.newEmail }}
          </p>
          <p v-if="emailServerError" role="alert" class="text-sm text-error">{{
            emailServerError
          }}</p>
          <p v-if="emailSent" role="status" class="break-keep text-sm text-muted">{{
            EMAIL_CHANGE_SENT_NOTICE
          }}</p>
        </form>

        <form class="flex flex-col gap-1.5" novalidate @submit.prevent="submitName">
          <label class="text-sm font-medium" for="settings-given-name">{{
            t("settings.givenName")
          }}</label>
          <div class="flex gap-2">
            <input
              id="settings-family-name"
              :class="['h-11 w-24 flex-none', fieldClass]"
              :aria-label="t('settings.familyName')"
              autocomplete="family-name"
              :value="nameForm.values.familyName"
              @input="nameForm.values.familyName = inputText($event)"
            />
            <input
              id="settings-given-name"
              :class="['h-11 min-w-0 flex-1', fieldClass]"
              autocomplete="given-name"
              :value="nameForm.values.givenName"
              @input="nameForm.values.givenName = inputText($event)"
            />
            <UButton type="submit" size="sm" :disabled="nameForm.submitting.value">
              {{
                nameForm.submitting.value
                  ? t("settings.profile.saving")
                  : t("settings.profile.save")
              }}
            </UButton>
          </div>
          <p v-if="nameFieldError()" role="alert" class="text-sm text-error">{{
            nameFieldError()
          }}</p>
          <p v-if="nameError" role="alert" class="text-sm text-error">{{ nameError }}</p>
        </form>

        <div class="flex flex-col gap-2">
          <p class="text-sm font-medium">{{ t("auth.password") }}</p>
          <form class="flex flex-col gap-1.5" novalidate @submit.prevent="submitPassword">
            <template v-if="me.hasPassword">
              <label class="text-sm font-medium" for="settings-current-password">{{
                t("auth.passwordCurrent")
              }}</label>
              <input
                id="settings-current-password"
                :class="fieldClass"
                type="password"
                autocomplete="current-password"
                :aria-invalid="passwordForm.errors.value.currentPassword ? true : undefined"
                :aria-describedby="
                  passwordForm.errors.value.currentPassword
                    ? 'settings-current-password-error'
                    : undefined
                "
                :value="passwordForm.values.currentPassword"
                @input="passwordForm.values.currentPassword = inputText($event)"
              />
              <p
                v-if="passwordForm.errors.value.currentPassword"
                id="settings-current-password-error"
                role="alert"
                class="text-sm text-error"
              >
                {{ passwordForm.errors.value.currentPassword }}
              </p>
            </template>
            <label class="text-sm font-medium" for="settings-new-password">
              {{ me.hasPassword ? t("auth.passwordNew") : t("auth.password") }}
            </label>
            <input
              id="settings-new-password"
              :class="fieldClass"
              type="password"
              autocomplete="new-password"
              :aria-invalid="passwordForm.errors.value.newPassword ? true : undefined"
              :aria-describedby="
                passwordForm.errors.value.newPassword ? 'settings-new-password-error' : undefined
              "
              :value="passwordForm.values.newPassword"
              @input="passwordForm.values.newPassword = inputText($event)"
            />
            <p
              v-if="passwordForm.errors.value.newPassword"
              id="settings-new-password-error"
              role="alert"
              class="text-sm text-error"
            >
              {{ passwordForm.errors.value.newPassword }}
            </p>
            <p v-if="passwordServerError" role="alert" class="text-sm text-error">{{
              passwordServerError
            }}</p>
            <p v-if="passwordSuccess" role="status" class="text-sm text-muted">{{
              PASSWORD_CHANGED_NOTICE
            }}</p>
            <UButton
              type="submit"
              size="sm"
              class="w-fit"
              :disabled="passwordForm.submitting.value"
            >
              {{
                passwordForm.submitting.value
                  ? t("form.changing")
                  : me.hasPassword
                    ? t("auth.reset.change")
                    : t("auth.account.password.create")
              }}
            </UButton>
          </form>
        </div>

        <slot name="mfa" />

        <div class="flex flex-col gap-2">
          <p class="text-sm font-medium">{{ t("auth.account.social.title") }}</p>
          <p v-if="providers.length === 0" class="text-sm text-muted">{{
            t("auth.account.social.empty")
          }}</p>
          <div
            v-for="p in providers"
            :key="p.provider"
            class="flex items-center justify-between gap-2 text-sm"
          >
            <span>
              {{ p.label }}
              <span v-if="linkedByProvider().get(p.provider)" class="text-muted">
                — {{ linkedByProvider().get(p.provider)?.email ?? t("common.connected") }}
              </span>
            </span>
            <ConfirmAction
              v-if="linkedByProvider().get(p.provider)"
              :title="t('auth.account.unlink.confirm.title')"
              :description="t('auth.account.unlink.confirm.body', { label: p.label })"
              :action-label="t('common.unlink')"
              :disabled="unlinkPending === p.provider"
              :on-confirm="() => handleUnlink(p.provider)"
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
              :aria-busy="linkPending === p.provider ? true : undefined"
              @click="linkProvider(p.provider)"
            >
              {{ t("common.link") }}
            </UButton>
          </div>
          <p v-if="methodError" role="alert" class="text-sm text-error">{{ methodError }}</p>
        </div>
      </div>
    </UPageCard>
    <UPageCard
      as="section"
      variant="subtle"
      class="settings-section"
      aria-labelledby="account-preferences-title"
    >
      <h2 id="account-preferences-title" class="settings-section__title">{{
        t("settings.title")
      }}</h2>
      <form class="flex flex-col gap-4" novalidate @submit.prevent="submitPreferences">
        <fieldset :disabled="preferenceForm.submitting.value" class="grid gap-4 sm:grid-cols-2">
          <legend class="sr-only">{{ t("settings.title") }}</legend>
          <UFormField
            name="locale"
            :label="t('settings.locale')"
            :error="preferenceForm.errors.value.locale"
          >
            <USelect
              id="settings-locale"
              class="w-full"
              :model-value="preferenceForm.values.locale"
              :items="[
                { label: settingLabel(optionKey('defaults.user', 'locale', 'ko')), value: 'ko' },
              ]"
              @update:model-value="
                (value) => {
                  if (value === 'ko') preferenceForm.values.locale = value;
                }
              "
            />
          </UFormField>
          <UFormField
            name="timezone"
            :label="t('settings.timezone')"
            :error="preferenceForm.errors.value.timezone"
          >
            <USelect
              id="settings-timezone"
              v-model="preferenceForm.values.timezone"
              class="w-full"
              :items="timezones"
            />
          </UFormField>
          <UFormField
            name="weekStartsOn"
            :label="t('settings.weekStart')"
            :error="preferenceForm.errors.value.weekStartsOn"
          >
            <USelect
              id="settings-week-start"
              v-model="preferenceForm.values.weekStartsOn"
              class="w-full"
              :items="
                SETTING_ENUM_OPTIONS['defaults.user.weekStartsOn']!.map((value) => ({
                  label: settingLabel(optionKey('defaults.user', 'weekStartsOn', value)),
                  value,
                }))
              "
            />
          </UFormField>
          <UFormField
            name="textScale"
            :label="t('settings.textScale')"
            :error="preferenceForm.errors.value.textScale"
          >
            <USelect
              id="settings-text-scale"
              v-model="preferenceForm.values.textScale"
              class="w-full"
              :items="
                SETTING_ENUM_OPTIONS['defaults.user.textScale']!.map((value) => ({
                  label: settingLabel(optionKey('defaults.user', 'textScale', value)),
                  value,
                }))
              "
            />
          </UFormField>
        </fieldset>
        <UFormField name="theme" :label="t('settings.theme')">
          <USelect
            id="settings-theme"
            class="w-full sm:max-w-xs"
            :model-value="theme"
            :items="[
              { label: t('settings.theme.system'), value: 'system' },
              { label: t('settings.theme.light'), value: 'light' },
              { label: t('settings.theme.dark'), value: 'dark' },
            ]"
            @update:model-value="changeTheme"
          />
        </UFormField>
        <p v-if="preferenceError" role="alert" class="text-sm text-error">{{ preferenceError }}</p>
        <p v-if="preferencesSaved" role="status" class="text-sm text-muted">{{
          t("common.saved")
        }}</p>
        <UButton type="submit" class="w-fit" :disabled="preferenceForm.submitting.value">{{
          t("settings.ui.save")
        }}</UButton>
      </form>
    </UPageCard>
    <UPageCard as="section" variant="subtle" class="settings-section">
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
      <p v-if="exportError" role="alert" class="settings-notice settings-notice--danger">{{
        exportError
      }}</p>
    </UPageCard>
    <UPageCard as="section" variant="subtle" class="settings-section">
      <h2 class="settings-section__title text-title">{{ t("auth.account.withdraw.title") }}</h2>
      <form class="flex flex-col gap-1.5" novalidate @submit.prevent="submitWithdraw">
        <label class="text-sm font-medium" for="withdraw-confirm">
          {{ me.hasPassword ? t("auth.passwordCurrent") : t("auth.account.withdraw.localPart") }}
        </label>
        <p class="break-keep text-sm text-muted">{{ t("auth.account.withdraw.body") }}</p>
        <p v-if="!me.hasPassword" class="break-keep text-sm text-muted">{{
          WITHDRAW_LOCAL_PART_HINT
        }}</p>
        <input
          id="withdraw-confirm"
          :class="fieldClass"
          :type="me.hasPassword ? 'password' : 'text'"
          :autocomplete="me.hasPassword ? 'current-password' : 'off'"
          :value="withdrawForm.values.confirmValue"
          @input="withdrawForm.values.confirmValue = inputText($event)"
        />
        <p
          v-if="withdrawForm.errors.value.confirmValue || withdrawError"
          role="alert"
          class="text-sm text-error"
        >
          {{ withdrawForm.errors.value.confirmValue ?? withdrawError }}
        </p>
        <UButton
          type="submit"
          size="sm"
          color="error"
          class="w-fit"
          :disabled="withdrawForm.submitting.value"
        >
          {{
            withdrawForm.submitting.value
              ? t("auth.withdraw.pending")
              : t("auth.account.withdraw.submit")
          }}
        </UButton>
      </form>
    </UPageCard>
  </div>
</template>
