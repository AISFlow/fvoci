<!--
  One cell of the summary row at the foot of the table: the result of the
  calculation chosen for its column. Pressing it opens the calculations the
  kind of the column can have.

  A column without a calculation says "Calculate". Where the device can
  hover that is shown only while the pointer is over the row; elsewhere it
  is always there.
-->
<script lang="ts">
  import ChevronUpIcon from '@lucide/svelte/icons/chevron-up';

  import type { MenuAction } from '$lib/components/menu/actions.ts';
  import ActionMenu from '$lib/components/menu/ActionMenu.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { CalculationOption } from '$lib/logic/properties.ts';

  import { calculationLabel, calculationShortName } from './calculation-names.ts';
  import { CELL_FOCUS, PINNED, ROW_HEIGHT } from './host.ts';

  interface Props {
    /** The identifier of the column. */
    id: string;
    /** The name of the column. */
    name: string;
    /** The title column: it stays at the left edge. */
    pinned?: boolean;
    option: CalculationOption;
    /** The result as text; empty without a calculation. */
    result: string;
    options: readonly CalculationOption[];
    /** The reader may change the view. */
    canEdit: boolean;
    onChoose: (option: CalculationOption) => void;
  }

  let { id, name, pinned = false, option, result, options, canEdit, onChoose }: Props = $props();

  let open = $state(false);

  const actions = $derived<MenuAction[]>(
    options.map((each) => ({
      id: each,
      label: calculationLabel(each),
      checked: each === option,
      run: () => {
        onChoose(each);
      },
    })),
  );
</script>

{#snippet cell(props: Record<string, unknown>)}
  <div
    {...props}
    role="gridcell"
    tabindex="-1"
    data-cell
    data-kind="calculation"
    data-col={id}
    data-calculation={option}
    data-pinned={pinned ? '' : undefined}
    class={[
      'group/calculation flex min-w-0 items-center justify-end gap-1.5 border-e border-t bg-background px-2 text-xs',
      ROW_HEIGHT,
      CELL_FOCUS,
      pinned && PINNED,
      canEdit && 'cursor-pointer hover:bg-muted aria-expanded:bg-muted',
    ]}
    onpointerdown={undefined}
    onpointerup={undefined}
    onkeydown={undefined}
    onclick={() => {
      if (canEdit) open = true;
    }}
  >
    {#if option === 'none'}
      {#if canEdit}
        <span
          class="flex items-center gap-1 text-muted-foreground can-hover:opacity-0 can-hover:group-hover/calculations:opacity-100 can-hover:group-focus-visible/calculation:opacity-100 can-hover:group-aria-expanded/calculation:opacity-100"
        >
          {calculationShortName(option)}
          <ChevronUpIcon class="size-3.5" aria-hidden="true" />
        </span>
      {/if}
    {:else}
      <span class="truncate text-muted-foreground" data-slot="calculation-name">
        {calculationShortName(option)}
      </span>
      <span class="shrink-0 font-medium tabular-nums" data-slot="calculation-result">{result}</span>
    {/if}
  </div>
{/snippet}

{#if canEdit}
  <ActionMenu
    bind:open
    title={m.table_calculation({ name })}
    {actions}
    align="start"
    trigger={cell}
  />
{:else}
  {@render cell({})}
{/if}
