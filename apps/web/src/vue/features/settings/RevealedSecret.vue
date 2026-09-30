<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import UInput from "@nuxt/ui/components/Input.vue";
import { ref } from "vue";
import { copyText } from "./clipboard";

const props = withDefaults(
  defineProps<{
    value: string;
    statusText: string;
    label: string;
    copyLabel?: string;
    copiedLabel?: string;
  }>(),
  { copyLabel: undefined, copiedLabel: undefined },
);

const copyStatus = ref<"copied" | "failed" | null>(null);

function copy(): void {
  void copyText(props.value).then(
    () => {
      copyStatus.value = "copied";
    },
    () => {
      copyStatus.value = "failed";
    },
  );
}
</script>

<template>
  <div class="flex flex-col gap-2 rounded-md border border-default p-3">
    <p role="status">{{ statusText }}</p>
    <div class="flex flex-wrap items-end gap-2">
      <UInput :model-value="value" readonly class="min-w-0 flex-1 font-mono" :aria-label="label" autocomplete="off" />
      <UButton type="button" size="sm" variant="outline" color="neutral" @click="copy">
        {{ copyStatus === "copied" ? (copiedLabel ?? t("token.copied")) : (copyLabel ?? t("token.copy")) }}
      </UButton>
    </div>
    <p v-if="copyStatus === 'failed'" role="alert" class="settings-notice settings-notice--danger">
      {{ t("workspace.invite.copyLink.failed") }}
    </p>
  </div>
</template>
