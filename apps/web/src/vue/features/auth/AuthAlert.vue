<script setup lang="ts">
defineProps<{ message: string }>();
</script>

<template>
  <p role="alert" class="auth-shell__alert">{{ message }}</p>
</template>
