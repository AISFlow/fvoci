<script setup lang="ts">
import { asSafeHtml, SafeHtml } from "@fvoci/editor/vue/safe-html";
import { t } from "@fvoci/i18n";
import { useQuery } from "@tanstack/vue-query";
import { computed } from "vue";
import { useRoute } from "vue-router";
import { loadErrorMessage, ProblemError } from "@/lib/api";
import { formatDateKo } from "@/lib/datetime";
import { legalDocQuery, legalVersionsQuery, selectLegalVersions } from "@/lib/queries/legal";
import QueryError from "../components/QueryError.vue";
import QueryLoading from "../components/QueryLoading.vue";
import AuthAlert from "../features/auth/AuthAlert.vue";
import AuthLayout from "../features/auth/AuthLayout.vue";
import AuthPanel from "../features/auth/AuthPanel.vue";

const route = useRoute();
const kind = computed(() => (typeof route.params.kind === "string" ? route.params.kind : ""));
const version = computed(() => {
  const raw = typeof route.query.version === "string" ? route.query.version : null;
  return raw && /^\d+$/.test(raw) ? Number(raw) : undefined;
});
const doc = useQuery(() => legalDocQuery(kind.value, version.value));
const versions = useQuery(() => ({
  ...legalVersionsQuery(kind.value),
  select: selectLegalVersions,
}));

const missing = computed(
  () => doc.error.value instanceof ProblemError && doc.error.value.status === 404,
);
const others = computed(() =>
  (versions.data.value ?? []).filter((entry) => entry.version !== doc.data.value?.version),
);
</script>

<template>
  <AuthLayout v-if="missing">
    <AuthPanel :title="kind">
      <AuthAlert :message="t('legal.empty')" />
    </AuthPanel>
  </AuthLayout>
  <AuthLayout v-else-if="doc.data.value === undefined">
    <QueryError
      v-if="doc.isError.value"
      :message="loadErrorMessage(doc.error.value)"
      @retry="() => void doc.refetch()"
    />
    <QueryLoading v-else />
  </AuthLayout>
  <AuthLayout v-else-if="doc.data.value">
    <AuthPanel
      :title="doc.data.value.title"
      :lead="
        t('legal.meta', {
          version: doc.data.value.version,
          date: formatDateKo(doc.data.value.effectiveAt),
        })
      "
    >
      <SafeHtml
        class="auth-shell__doc-body break-keep"
        :html="asSafeHtml(doc.data.value.bodyHtml)"
      />
      <div v-if="others.length > 0" class="auth-shell__stack border-t border-default pt-5">
        <p class="auth-shell__text auth-shell__text--strong">{{ t("legal.previous") }}</p>
        <a
          v-for="entry in others"
          :key="entry.version"
          :href="`/legal/${kind}?version=${entry.version}`"
          class="auth-shell__link break-keep"
        >
          v{{ entry.version }}({{ formatDateKo(entry.effectiveAt) }})
        </a>
      </div>
    </AuthPanel>
  </AuthLayout>
</template>
