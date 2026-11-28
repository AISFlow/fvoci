<!--
  Whether a card has to meet every part of a group of a filter, or any one
  of them: two choices side by side, one of them chosen; the arrows go from
  one to the other. The filter itself has one, and so has each group in it.
-->
<script lang="ts">
  import { arrowChoice } from '$lib/components/arrow-choice.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { FilterGroupOperation } from '$lib/logic/wire.ts';

  interface Props {
    operation: FilterGroupOperation;
    /** Names the choice for a screen reader: of the filter, or of a group. */
    label: string;
    onChange: (operation: FilterGroupOperation) => void;
  }

  let { operation, label, onChange }: Props = $props();

  const OPERATIONS: readonly { operation: FilterGroupOperation; label: () => string }[] = [
    { operation: 'and', label: m.viewheader_filter_match_all },
    { operation: 'or', label: m.viewheader_filter_match_any },
  ];
</script>

<div
  role="radiogroup"
  aria-label={label}
  {@attach arrowChoice()}
  class="flex max-w-full gap-1 self-start rounded-lg bg-muted p-0.5"
>
  {#each OPERATIONS as choice (choice.operation)}
    <button
      type="button"
      role="radio"
      aria-checked={operation === choice.operation}
      data-operation={choice.operation}
      class="h-7 min-w-0 truncate rounded-md px-2.5 text-sm font-medium text-muted-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring aria-checked:bg-background aria-checked:text-foreground aria-checked:shadow-sm roomy:h-touch roomy:px-3 roomy:text-base"
      onclick={() => {
        if (operation !== choice.operation) onChange(choice.operation);
      }}
    >
      {choice.label()}
    </button>
  {/each}
</div>
