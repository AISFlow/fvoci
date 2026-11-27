<!--
  Renders a `LazyChunk` once its module arrives, showing the standard loading
  placeholder on first fetch and a retry button if the network dropped.
-->
<script lang="ts" generics="T">
  import type { Snippet } from 'svelte';

  import type { LazyChunk } from '$lib/chunk-reload.svelte.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { Button } from '$lib/ui/button/index.js';

  interface Props {
    chunk: LazyChunk<T>;
    children: Snippet<[T]>;
  }

  let { chunk, children }: Props = $props();

  $effect.pre(() => {
    chunk.ensure();
  });
</script>

{#if chunk.value}
  {@render children(chunk.value)}
{:else if chunk.failed}
  <div class="grid h-full place-items-center p-6 text-center">
    <div>
      <p class="text-muted-foreground" role="alert">{m.app_problem_load()}</p>
      <Button
        class="mt-4 roomy:min-h-touch"
        onclick={() => {
          chunk.ensure();
        }}
      >
        {m.app_retry()}
      </Button>
    </div>
  </div>
{:else}
  <div class="grid h-full place-items-center p-6">
    <p class="text-muted-foreground" role="status">{m.app_loading()}</p>
  </div>
{/if}
