<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useId, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import type { NotificationItem } from "@/features/notifications/notification-target";
import { loadErrorMessage } from "@/lib/api";
import { notificationsPath } from "@/lib/href";
import QueryError from "../../components/QueryError.vue";
import { useNotificationBell } from "./useNotificationBell";

// The header bell (features/notifications/notification-bell.tsx): a button
// named by the unread count that opens the latest notifications below it.
const props = defineProps<{ slug: string; workspaceId: string }>();
const router = useRouter();
const panelId = useId();
const bell = useNotificationBell({
  workspaceId: () => props.workspaceId,
  slug: () => props.slug,
  // A task or document is a React page (the router loads it) or a wiki
  // document of this app.
  navigate: (path) => void router.push(path),
});
const { open, count, items, label, badge } = bell;
const { isPending: listPending, isError: listFailed, error: listError } = bell.list;

// An in-app navigation (to another wiki document) keeps the shell mounted;
// the panel closes with it, as on a new page. Query-only replaces (the
// Gantt month) stay on this page, so they leave the panel open.
const route = useRoute();
watch(
  () => route.path,
  () => {
    open.value = false;
  },
);

// As in the React bell, a failed write leaves the panel as it was.
function onItem(item: NotificationItem): void {
  bell.openItem(item).catch(() => undefined);
}

function onReadAll(): void {
  bell.readAll().catch(() => undefined);
}
</script>

<template>
  <!-- On narrow screens, the wrapping shell header anchors the panel. -->
  <div class="sm:relative">
    <UButton
      color="neutral"
      variant="outline"
      square
      icon="i-lucide-bell"
      class="relative"
      :aria-label="label"
      :aria-expanded="open"
      :aria-controls="open ? panelId : undefined"
      @click="open = !open"
    >
      <span
        v-if="count > 0"
        aria-hidden="true"
        class="absolute -top-1.5 -right-1.5 min-w-4 rounded-full bg-error px-1 text-center text-[0.6875rem] leading-4 text-inverted"
        >{{ badge }}</span
      >
    </UButton>
    <div
      v-if="open"
      :id="panelId"
      role="region"
      :aria-label="t('notif.bell.title')"
      class="absolute top-full z-20 mt-1.5 w-[min(22rem,calc(100vw-1.5rem))] rounded-xl border border-default bg-default shadow-lg max-sm:inset-x-3 max-sm:w-auto sm:right-0"
    >
      <div class="flex items-center justify-between gap-2 border-b border-default px-3 py-2">
        <p class="m-0 text-sm font-semibold">{{ t("notif.bell.title") }}</p>
        <UButton v-if="count > 0" size="sm" variant="outline" color="neutral" @click="onReadAll">{{
          t("notif.readAll")
        }}</UButton>
      </div>
      <p v-if="listPending" class="m-0 p-3 text-sm text-muted">{{ t("load.loading") }}</p>
      <div v-if="listFailed" class="p-3">
        <QueryError :message="loadErrorMessage(listError)" @retry="bell.list.refetch()" />
      </div>
      <p
        v-if="!listPending && !listFailed && items.length === 0"
        class="m-0 p-3 text-sm text-muted"
      >
        {{ t("notif.empty") }}
      </p>
      <ul
        v-if="items.length > 0"
        class="m-0 max-h-[min(24rem,60vh)] list-none overflow-auto px-0 py-1"
      >
        <li v-for="item in items" :key="item.id">
          <button
            type="button"
            class="flex w-full items-start gap-2 px-3 py-2 text-start text-sm break-keep hover:bg-elevated"
            @click="onItem(item)"
          >
            <span
              aria-hidden="true"
              :class="[
                'mt-1.5 size-1.5 shrink-0 rounded-full',
                item.readAt ? 'bg-transparent' : 'bg-primary',
              ]"
            />
            <span>{{ bell.message(item) }}</span>
          </button>
        </li>
      </ul>
      <div class="flex items-center gap-2 border-t border-default px-3 py-2 text-sm">
        <a :href="notificationsPath(slug)" class="underline underline-offset-2">{{
          t("notif.viewAll")
        }}</a>
      </div>
    </div>
  </div>
</template>
