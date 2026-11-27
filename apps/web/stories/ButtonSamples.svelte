<script lang="ts">
  import { Button } from '$lib/ui/button/index.js';

  let { disabled = false }: { disabled?: boolean } = $props();

  const variants = ['default', 'outline', 'secondary', 'ghost', 'destructive', 'link'] as const;
</script>

<div class="flex flex-wrap items-center gap-3">
  {#each variants as variant (variant)}
    <Button {variant} {disabled}>{variant} · 저장</Button>
  {/each}
</div>
