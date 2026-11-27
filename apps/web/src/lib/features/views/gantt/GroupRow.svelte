<!--
  The row that starts a group of the chart: the value the group stands for
  and how many of its cards have a bar. What it says stays at the left edge
  while the days scroll. The view is grouped as a table is (`groupBy`).
-->
<script lang="ts">
  import OptionChip from '$lib/components/option-chip/OptionChip.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';

  import type { GroupLine } from './chart.svelte.ts';

  let { line }: { line: GroupLine } = $props();
</script>

<li
  data-group={line.id}
  class="relative z-1 flex h-(--gantt-row) border-b border-border/60 bg-muted/50"
>
  <div class="sticky left-0 flex max-w-64 min-w-0 items-center gap-1.5 px-3">
    {#if line.color === undefined}
      <span class="min-w-0 truncate font-medium" data-slot="group-name">{line.name}</span>
    {:else}
      <span class="min-w-0" data-slot="group-name">
        <OptionChip name={line.name} color={line.color} />
      </span>
    {/if}
    <span class="text-xs text-muted-foreground tabular-nums" data-slot="group-count">
      <span aria-hidden="true">{line.count}</span>
      <span class="sr-only">{m.table_group_count({ count: line.count })}</span>
    </span>
  </div>
</li>
