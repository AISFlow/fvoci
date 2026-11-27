<script lang="ts">
  import type { Snippet } from 'svelte';

  let { locale, children }: { locale: string; children: Snippet } = $props();
</script>

<!-- Paraglide's global locale is not a Svelte signal. Recreate locale-dependent
     formatters when the toolbar changes it, including in the existing stories. -->
{#key locale}
  {@render children()}
{/key}
