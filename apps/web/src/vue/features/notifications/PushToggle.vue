<script setup lang="ts">
import { t } from "@fvoci/i18n";
import { useQuery } from "@tanstack/vue-query";
import { computed, ref, watch } from "vue";
import { pushSupported } from "@/features/notifications/push-browser";
import {
  liveSubscribed,
  refreshOwnSubscription,
  subscribePush,
  unsubscribePush,
} from "@/features/notifications/push-enable";
import { attempted, type PushAttempted, type PushBlocker, pushBlocker } from "@/features/notifications/push-subscription";
import { meQuery } from "@/lib/queries";
import { publicInstanceQuery } from "@/lib/queries/instance";

const props = defineProps<{ workspaceId: string }>();

const instance = useQuery(publicInstanceQuery);
const me = useQuery(meQuery);
const userId = computed(() => me.data.value?.userId ?? null);
const publicKey = computed(() => instance.data.value?.values.webPushPublicKey ?? null);
const enabled = ref(false);
const failure = ref<PushAttempted | null>(null);
const busy = ref(false);

watch(
  () => [props.workspaceId, publicKey.value, userId.value] as const,
  ([workspaceId, key, uid], _previous, onCleanup) => {
    if (!pushSupported() || key === null || uid === null) return;
    let live = true;
    onCleanup(() => {
      live = false;
    });
    void (async () => {
      await refreshOwnSubscription(workspaceId, key, uid);
      if (live) enabled.value = await liveSubscribed(key, uid);
    })().catch(async (err: unknown) => {
      if (!live) return;
      failure.value = attempted(err);
      enabled.value = await liveSubscribed(key, uid).catch(() => false);
    });
  },
  { immediate: true },
);

const reason = computed<PushBlocker | null>(() =>
  pushBlocker({
    supported: pushSupported(),
    instanceLoaded: instance.isSuccess.value,
    publicKey: publicKey.value,
    attempted: failure.value,
  }),
);
const unavailable = computed(() => reason.value === "unsupported" || reason.value === "unavailable");

function pushReasonMessage(value: PushBlocker): string {
  switch (value) {
    case "unsupported":
      return t("notif.push.unsupported");
    case "unavailable":
      return t("notif.push.unavailable");
    case "blocked":
      return t("notif.push.blocked");
    case "failed":
      return t("notif.push.failed");
  }
}

function toggle(next: boolean): void {
  const key = publicKey.value;
  const uid = userId.value;
  if (key === null || uid === null) return;
  busy.value = true;
  failure.value = null;
  void (async () => {
    try {
      if (next) await subscribePush(props.workspaceId, key, uid);
      else await unsubscribePush(uid);
    } catch (err) {
      failure.value = attempted(err);
    } finally {
      enabled.value = await liveSubscribed(key, uid).catch(() => false);
      busy.value = false;
    }
  })();
}
</script>

<template>
  <label class="settings-form__row">
    <input
      id="prefs-push"
      type="checkbox"
      :checked="enabled"
      :disabled="unavailable || busy || userId === null"
      @change="toggle(($event.target as HTMLInputElement).checked)"
    />
    <span>{{ t("notif.prefs.push") }}</span>
  </label>
  <p class="settings-notice">{{ t("notif.prefs.pushHint") }}</p>
  <p v-if="reason" role="alert" class="settings-notice settings-notice--danger">{{ pushReasonMessage(reason) }}</p>
</template>
