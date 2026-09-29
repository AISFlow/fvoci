<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import UInput from "@nuxt/ui/components/Input.vue";
import { computed, ref } from "vue";
import type { components } from "@/generated/api";
import { problemMessage } from "@/lib/api";
import { qrModules } from "@/lib/qr";

type MfaStatusOutput = components["schemas"]["MfaStatusOutput"];
type MfaSetupOutput = components["schemas"]["MfaSetupOutput"];
type MfaSetupInput = components["schemas"]["MfaSetupBody"];
type MfaDisableInput = components["schemas"]["MfaDisableBody"];

const props = defineProps<{
  status: MfaStatusOutput;
  hasPassword: boolean;
  onSetup: (input: MfaSetupInput) => Promise<MfaSetupOutput>;
  onEnable: (code: string) => Promise<void>;
  onDisable: (input: MfaDisableInput) => Promise<void>;
}>();

const setup = ref<MfaSetupOutput | null>(null);
const notice = ref<string | null>(null);
const enabledFlow = ref(false);
const copied = ref(false);
const enableError = ref<string | null>(null);
const enableCode = ref("");
const enableBusy = ref(false);
const reauthValue = ref("");
const reauthError = ref<string | null>(null);
const reauthBusy = ref(false);

const qr = computed(() => (setup.value ? qrModules(setup.value.otpauthUri) : null));

async function copyText(value: string): Promise<void> {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(value);
    return;
  }
  throw new Error("clipboard unavailable");
}

function doneSetup(): void {
  setup.value = null;
  enabledFlow.value = false;
  enableCode.value = "";
  enableError.value = null;
}

async function confirmEnable(): Promise<void> {
  enableError.value = null;
  enableBusy.value = true;
  try {
    await props.onEnable(enableCode.value.trim());
    enabledFlow.value = true;
  } catch (err) {
    enableError.value = problemMessage(err, "error.auth.mfa");
  } finally {
    enableBusy.value = false;
  }
}

async function submitReauth(): Promise<void> {
  reauthError.value = null;
  reauthBusy.value = true;
  const label = props.status.enabled
    ? props.hasPassword
      ? t("auth.passwordCurrent")
      : t("auth.account.mfa.disable.code")
    : props.hasPassword
      ? t("auth.passwordCurrent")
      : null;
  try {
    const value = label === null ? null : reauthValue.value.trim();
    if (props.status.enabled) {
      await props.onDisable(
        props.hasPassword ? { currentPassword: value, code: null } : { currentPassword: null, code: value },
      );
      notice.value = t("auth.account.mfa.disabled");
      reauthValue.value = "";
    } else {
      notice.value = null;
      setup.value = await props.onSetup({ currentPassword: value });
      reauthValue.value = "";
    }
  } catch (err) {
    reauthError.value = problemMessage(err, props.status.enabled ? "error.mfa.disable" : "error.mfa.setup");
  } finally {
    reauthBusy.value = false;
  }
}

const reauthLabel = computed(() => {
  if (setup.value) return null;
  if (props.status.enabled) return props.hasPassword ? t("auth.passwordCurrent") : t("auth.account.mfa.disable.code");
  return props.hasPassword ? t("auth.passwordCurrent") : null;
});
const reauthAction = computed(() =>
  props.status.enabled ? t("auth.account.mfa.disable") : t("auth.account.mfa.setup"),
);
</script>

<template>
  <div class="flex flex-col gap-2" data-testid="mfa-section">
    <h2 class="text-sm font-medium">{{ t("auth.mfa.title") }}</h2>
    <p class="text-sm text-muted" data-testid="mfa-status">
      {{
        status.enabled
          ? t("auth.account.mfa.on", { count: status.recoveryCodesLeft })
          : t("auth.account.mfa.off")
      }}
    </p>
    <p v-if="notice" role="status" class="text-sm text-muted">{{ notice }}</p>
    <div v-if="setup && enabledFlow" class="flex flex-col gap-2">
      <p role="status" class="text-sm font-medium">{{ t("auth.account.mfa.enabled") }}</p>
      <p class="text-sm font-medium">{{ t("auth.account.mfa.recovery.title") }}</p>
      <p class="break-keep text-sm text-muted">{{ t("auth.account.mfa.recovery.body") }}</p>
      <ul class="grid grid-cols-2 gap-x-6 gap-y-1 font-mono text-sm" data-testid="mfa-recovery-codes">
        <li v-for="code in setup.recoveryCodes" :key="code">{{ code }}</li>
      </ul>
      <div class="flex gap-2">
        <UButton
          type="button"
          variant="outline"
          color="neutral"
          size="sm"
          @click="
            copyText(setup.recoveryCodes.join('\n')).then(
              () => (copied = true),
              () => (copied = false),
            )
          "
        >
          {{ copied ? t("auth.account.mfa.recovery.copied") : t("auth.account.mfa.recovery.copy") }}
        </UButton>
        <UButton type="button" size="sm" @click="doneSetup">{{ t("auth.account.mfa.recovery.done") }}</UButton>
      </div>
    </div>
    <form v-else-if="setup && qr" class="flex flex-col gap-2" novalidate @submit.prevent="confirmEnable">
      <p class="break-keep text-sm text-muted">{{ t("auth.account.mfa.scan") }}</p>
      <div class="flex flex-wrap items-start gap-4">
        <a :href="setup.otpauthUri" class="shrink-0" :aria-label="t('auth.account.mfa.scan')" data-testid="mfa-otpauth-uri">
          <svg
            :viewBox="`-2 -2 ${qr.size + 4} ${qr.size + 4}`"
            shape-rendering="crispEdges"
            class="size-44 rounded-md bg-white"
            aria-hidden="true"
            data-testid="mfa-qr"
          >
            <title>QR</title>
            <path :d="qr.path" fill="#000" />
          </svg>
        </a>
        <div class="flex min-w-0 flex-col gap-1.5">
          <p class="text-sm font-medium">{{ t("auth.account.mfa.manualKey") }}</p>
          <code class="break-all font-mono text-sm" data-testid="mfa-secret">{{ setup.secret }}</code>
        </div>
      </div>
      <label for="settings-mfa-code">{{ t("auth.mfa.code") }}</label>
      <UInput id="settings-mfa-code" v-model="enableCode" autocomplete="one-time-code" inputmode="numeric" />
      <p v-if="enableError" role="alert" class="text-sm text-error">{{ enableError }}</p>
      <div class="flex gap-2">
        <UButton type="submit" size="sm" :disabled="enableBusy">
          {{ enableBusy ? t("auth.mfa.setup.confirming") : t("auth.account.mfa.enable") }}
        </UButton>
        <UButton type="button" variant="outline" color="neutral" size="sm" @click="doneSetup">{{
          t("auth.mfa.setup.cancel")
        }}</UButton>
      </div>
    </form>
    <form v-else class="flex flex-col gap-1.5" novalidate @submit.prevent="submitReauth">
      <label v-if="reauthLabel" for="settings-mfa-confirm">{{ reauthLabel }}</label>
      <div class="flex gap-2">
        <UInput
          v-if="reauthLabel"
          id="settings-mfa-confirm"
          v-model="reauthValue"
          :type="hasPassword ? 'password' : 'text'"
          :autocomplete="hasPassword ? 'current-password' : 'one-time-code'"
        />
        <UButton type="submit" variant="outline" color="neutral" size="sm" :disabled="reauthBusy">
          {{ reauthBusy ? t("auth.mfa.reauth.pending") : reauthAction }}
        </UButton>
      </div>
      <p v-if="reauthError" role="alert" class="text-sm text-error">{{ reauthError }}</p>
    </form>
  </div>
</template>
