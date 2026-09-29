<script setup lang="ts">
import { t } from "@fvoci/i18n";
import { RouterLink } from "vue-router";
import { useQuery } from "@tanstack/vue-query";
import { computed, watchEffect } from "vue";
import { meQuery } from "@/lib/queries";
import { loginPath, redirectTo } from "../session/navigation";
import QueryLoading from "./QueryLoading.vue";
import "@/features/settings/settings-shell.css";

export type AdminNav = "admin" | "audit" | "legal";

const props = defineProps<{ active: AdminNav }>();

const NAV: { key: AdminNav; to: string; labelKey: "settings.admin" | "audit.title" | "settings.legal" }[] = [
  { key: "admin", to: "/settings/admin", labelKey: "settings.admin" },
  { key: "audit", to: "/settings/audit", labelKey: "audit.title" },
  { key: "legal", to: "/settings/legal", labelKey: "settings.legal" },
];

// Source `beforeLoad`: signed-out → /login; not an instance admin → /.
const me = useQuery(meQuery);
const ready = computed(() => me.data.value);
watchEffect(() => {
  if (me.isError.value) redirectTo(loginPath(window.location));
  else if (me.data.value && !me.data.value.isInstanceAdmin) redirectTo("/");
});
</script>

<template>
  <QueryLoading v-if="me.isLoading.value || !ready" />
  <div v-else-if="ready.isInstanceAdmin" class="flex min-h-screen flex-col">
    <header class="flex flex-wrap items-center justify-between gap-3 border-b border-default px-4 py-3">
      <div class="flex flex-wrap items-center gap-4">
        <a href="/" class="underline underline-offset-2">{{ t("nav.backHome") }}</a>
        <nav class="flex flex-wrap items-center gap-3" :aria-label="t('admin.console')">
          <RouterLink
            v-for="item in NAV"
            :key="item.key"
            :to="item.to"
            :class="props.active === item.key ? 'font-medium text-highlighted' : undefined"
            :aria-current="props.active === item.key ? 'page' : undefined"
            >{{ t(item.labelKey) }}</RouterLink
          >
        </nav>
      </div>
    </header>
    <main class="flex-1 p-4">
      <div class="settings-page">
        <slot />
      </div>
    </main>
  </div>
</template>
