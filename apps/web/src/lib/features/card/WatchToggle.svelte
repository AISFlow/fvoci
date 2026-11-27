<!--
  "지켜보기" at the head of a card or a page: pressed, the reader is told when
  someone comments on it or changes it (`BlockWatch`). Whoever wrote it or
  commented on it watches it already, and may stop. Not shown where nothing
  is watched (the own space, a public link) or until it is known. What is
  watched is read once for the workspace, not for each card or page; the
  reading follows the block alone, not the reader's account as it is read
  again (a connection that comes back reads the reader anew).
-->
<script lang="ts">
  import BellIcon from '@lucide/svelte/icons/bell';
  import BellOffIcon from '@lucide/svelte/icons/bell-off';
  import { untrack } from 'svelte';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { BlockWatch, type Workspace } from '$lib/state/index.ts';

  interface Props {
    workspace: Workspace;
    blockId: string;
    kind: 'card' | 'page';
  }

  let { workspace, blockId, kind }: Props = $props();

  const watch = $derived(new BlockWatch(workspace, blockId, kind));

  $effect(() => {
    const shown = watch;
    untrack(() => void shown.read());
  });
</script>

{#if watch.canWatch && watch.watching !== undefined}
  <IconButton
    label={m.watch_label()}
    aria-pressed={watch.watching}
    disabled={watch.busy}
    data-testid="watch-toggle"
    class={watch.watching ? 'text-primary' : 'text-muted-foreground'}
    onclick={() => void watch.set(!watch.watching)}
  >
    {#if watch.watching}
      <BellIcon aria-hidden="true" />
    {:else}
      <BellOffIcon aria-hidden="true" />
    {/if}
  </IconButton>
{/if}
