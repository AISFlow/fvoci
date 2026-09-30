<script setup lang="ts">
import { t } from "@fvoci/i18n";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref } from "vue";
import { api, ensureOk, loadErrorMessage, ProblemError } from "@/lib/api";
import { notificationPrefsQuery } from "@/lib/queries";
import QueryError from "../../components/QueryError.vue";
import QueryLoading from "../../components/QueryLoading.vue";
import PushToggle from "../notifications/PushToggle.vue";
import "@/features/settings/settings-shell.css";

const props = defineProps<{ workspaceId: string }>();
const queryClient = useQueryClient();
const prefsQuery = useQuery(() => notificationPrefsQuery(props.workspaceId));
const save = useMutation({
  mutationFn: async (body: { inApp: boolean; mailImmediate: boolean; mailDigest: boolean }) =>
    ensureOk(
      await api.PUT("/api/v1/workspaces/{workspace_id}/notification-prefs", {
        params: { path: { workspace_id: props.workspaceId } },
        body,
      }),
    ),
  onSuccess: async () => {
    await queryClient.invalidateQueries({ queryKey: ["notification-prefs", props.workspaceId] });
    await queryClient.invalidateQueries({ queryKey: ["notifications", props.workspaceId] });
    await queryClient.invalidateQueries({ queryKey: ["notifications-unread", props.workspaceId] });
  },
});
const prefs = computed(() => prefsQuery.data.value);
const saving = ref(false);

async function changePreference(key: "inApp" | "mailImmediate" | "mailDigest", event: Event): Promise<void> {
  const input = event.target as HTMLInputElement;
  const current = prefs.value;
  if (!current) return;
  const next = input.checked;
  input.checked = current[key];
  if (saving.value) return;
  saving.value = true;
  try {
    await save.mutateAsync({
      inApp: current.inApp,
      mailImmediate: current.mailImmediate,
      mailDigest: current.mailDigest,
      [key]: next,
    });
  } catch {
    // The mutation renders the problem below and keeps the committed value.
  } finally {
    saving.value = false;
    input.checked = prefs.value?.[key] ?? current[key];
  }
}
</script>

<template>
  <QueryLoading v-if="prefsQuery.isLoading.value" />
  <QueryError
    v-else-if="prefsQuery.isError.value"
    :message="loadErrorMessage(prefsQuery.error.value)"
    @retry="prefsQuery.refetch()"
  />
  <section v-else-if="prefs" class="settings-section">
    <h2 class="settings-section__title">{{ t("settings.notifications.title") }}</h2>
    <fieldset class="flex flex-col gap-3" :disabled="saving" :aria-busy="saving">
      <legend class="sr-only">{{ t("settings.notifications.title") }}</legend>
    <label class="settings-form__row">
      <input
        id="prefs-in-app"
        type="checkbox"
        :checked="prefs.inApp"
        @change="changePreference('inApp', $event)"
      />
      <span>{{ t("notif.prefs.inApp") }}</span>
    </label>
    <label class="settings-form__row">
      <input
        id="prefs-mail-immediate"
        type="checkbox"
        :checked="prefs.mailImmediate"
        @change="changePreference('mailImmediate', $event)"
      />
      <span>{{ t("notif.prefs.mailImmediate") }}</span>
    </label>
    <label class="settings-form__row">
      <input
        id="prefs-mail-digest"
        type="checkbox"
        :checked="prefs.mailDigest"
        @change="changePreference('mailDigest', $event)"
      />
      <span>{{ t("notif.prefs.mailDigest") }}</span>
    </label>
    </fieldset>
    <PushToggle :workspace-id="workspaceId" />
    <p v-if="save.error.value" role="alert" class="settings-notice">
      {{ save.error.value instanceof ProblemError ? save.error.value.title : t("settings.save.failed") }}
    </p>
  </section>
</template>
