<script setup lang="ts">
defineProps<{ message: string }>();
</script>

<template>
  <p role="status" class="auth-shell__status break-keep">{{ message }}</p>
</template>
