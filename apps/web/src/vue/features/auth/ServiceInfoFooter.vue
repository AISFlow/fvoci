<script setup lang="ts">
import { t } from "@fvoci/i18n";
import { hasOperatorInfo, LEGAL_DOCS, type OperatorInfo } from "@/features/legal/operator-fields";

defineProps<{ operator: OperatorInfo | null }>();

// The build writes the notice; the dev server has none.
const noticeShipped = import.meta.env.PROD;
</script>

<template>
  <!-- Public Vue pages use full loads to start with fresh queries. -->
  <footer class="auth-shell__footer">
    <a v-if="hasOperatorInfo(operator)" class="auth-shell__footer-link" href="/service-info">
      {{ t("operator.title") }}
    </a>
    <a v-for="doc in LEGAL_DOCS" :key="doc.kind" class="auth-shell__footer-link" :href="`/legal/${doc.kind}`">
      {{ doc.label }}
    </a>
    <a v-if="noticeShipped" class="auth-shell__footer-link" href="/open-source-licenses.txt">
      {{ t("legal.openSourceNotices") }}
    </a>
  </footer>
</template>
