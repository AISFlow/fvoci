<script setup lang="ts">
defineProps<{ title: string; lead?: string }>();
</script>

<template>
  <section class="auth-shell__panel" aria-labelledby="auth-panel-title">
    <header>
      <h1 id="auth-panel-title" class="auth-shell__heading">{{ title }}</h1>
      <p v-if="lead" class="auth-shell__lead">{{ lead }}</p>
    </header>
    <div class="auth-shell__stack auth-shell__stack--form mt-6">
      <slot />
    </div>
  </section>
</template>
