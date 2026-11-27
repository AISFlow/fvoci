<!--
  The heading of a part of a panel that folds (`Fold`): one button with the
  part's name, and how many it holds where there are any, that shows and hides
  what is under it (`controls`, the element that holds it while it shows).
-->
<script lang="ts">
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';

  import type { Fold } from './fold.svelte.ts';

  interface Props {
    id: string;
    fold: Fold;
    controls: string;
    title: string;
    count?: number | undefined;
    testId?: string | undefined;
  }

  let { id, fold, controls, title, count, testId }: Props = $props();
</script>

<h3 {id} class="font-medium">
  <button
    type="button"
    class="flex min-h-8 w-full items-center gap-1.5 rounded-sm text-start outline-none focus-visible:ring-3 focus-visible:ring-ring/50 roomy:min-h-touch"
    aria-expanded={fold.open}
    aria-controls={fold.open ? controls : undefined}
    data-testid={testId}
    onclick={() => {
      fold.toggle();
    }}
  >
    <ChevronRightIcon
      class={[
        'size-4 shrink-0 text-muted-foreground transition-transform motion-reduce:transition-none',
        fold.open && 'rotate-90',
      ]}
      aria-hidden="true"
    />
    <span class="min-w-0 truncate">{title}</span>
    {#if count}
      <span class="text-sm font-normal text-muted-foreground tabular-nums">({count})</span>
    {/if}
  </button>
</h3>
