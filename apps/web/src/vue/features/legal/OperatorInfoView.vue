<script setup lang="ts">
import { t, type I18nKey } from "@fvoci/i18n";
import {
  filledOperatorFields,
  hasOperatorInfo,
  operatorFieldHref,
  type OperatorField,
  type OperatorInfo,
} from "@/features/legal/operator-fields";
import AuthLayout from "../auth/AuthLayout.vue";
import AuthPanel from "../auth/AuthPanel.vue";
import AuthStatus from "../auth/AuthStatus.vue";

const props = defineProps<{ operator: OperatorInfo | null }>();

function rows(operator: OperatorInfo | null): {
  field: OperatorField;
  value: string;
  href: string | null;
  label: string;
}[] {
  return filledOperatorFields(operator).map((field) => {
    const value = operator?.[field] ?? "";
    return {
      field,
      value,
      href: operatorFieldHref(field, value),
      label: t(`operator.${field}` as I18nKey),
    };
  });
}
</script>

<template>
  <AuthLayout width="wide" :show-wordmark="false">
    <AuthPanel :title="t('operator.title')">
      <AuthStatus v-if="!hasOperatorInfo(props.operator)" :message="t('operator.empty')" />
      <dl v-else class="auth-shell__operator">
        <template v-for="row in rows(props.operator)" :key="row.field">
          <dt>{{ row.label }}</dt>
          <dd>
            <a
              v-if="row.href"
              class="auth-shell__link break-all"
              :href="row.href"
              rel="noreferrer"
              :target="row.field === 'businessInfoUrl' ? '_blank' : undefined"
            >
              {{ row.value }}
            </a>
            <template v-else>{{ row.value }}</template>
          </dd>
        </template>
      </dl>
    </AuthPanel>
  </AuthLayout>
</template>
