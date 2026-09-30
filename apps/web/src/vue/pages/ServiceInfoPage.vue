<script setup lang="ts">
import { useQuery } from "@tanstack/vue-query";
import { loadErrorMessage } from "@/lib/api";
import { publicInstanceQuery } from "@/lib/queries/instance";
import QueryError from "../components/QueryError.vue";
import QueryLoading from "../components/QueryLoading.vue";
import AuthLayout from "../features/auth/AuthLayout.vue";
import OperatorInfoView from "../features/legal/OperatorInfoView.vue";

// Fail once and show QueryError + manual retry; avoid TanStack retry delay on this public page.
const instance = useQuery(() => ({ ...publicInstanceQuery, retry: false }));
</script>

<template>
  <AuthLayout v-if="instance.data.value === undefined" width="wide" :show-wordmark="false">
    <QueryError
      v-if="instance.isError.value"
      :message="loadErrorMessage(instance.error.value)"
      @retry="() => void instance.refetch()"
    />
    <QueryLoading v-else />
  </AuthLayout>
  <OperatorInfoView v-else :operator="instance.data.value.values.operator" />
</template>
