<!--
  The sort of a view: a list of properties, each ascending or descending. The
  cards are ordered by the first line; cards the first line leaves equal by
  the second, and so on, which is why the lines can be moved up and down.

  Every change is saved as it is made. The focus follows the line that was
  moved, and goes to "Add sort" when a line is removed, so that the keyboard
  is never left on a button that is gone.
-->
<script lang="ts">
  import ArrowDownIcon from '@lucide/svelte/icons/arrow-down';
  import ArrowDownWideNarrowIcon from '@lucide/svelte/icons/arrow-down-wide-narrow';
  import ArrowUpIcon from '@lucide/svelte/icons/arrow-up';
  import ArrowUpNarrowWideIcon from '@lucide/svelte/icons/arrow-up-narrow-wide';
  import PlusIcon from '@lucide/svelte/icons/plus';
  import XIcon from '@lucide/svelte/icons/x';
  import { tick } from 'svelte';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import ActionMenu from '$lib/components/menu/ActionMenu.svelte';
  import type { MenuAction } from '$lib/components/menu/actions.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { BoardView, SortOption } from '$lib/logic/wire.ts';
  import type { BoardStore } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import { addSort, moveSort, removeSort, reverseSort, sortCandidates } from './sort-edit.ts';
  import { propertyName } from './texts.ts';

  let { board, view }: { board: BoardStore; view: BoardView } = $props();

  const templates = $derived(board.board?.cardProperties ?? []);
  const options = $derived(view.fields.sort);
  /** The views whose cards are dragged into an order: a sort takes that away. */
  const dragged = $derived(view.fields.viewType === 'board' || view.fields.viewType === 'table');

  let root = $state<HTMLElement | null>(null);
  /** What a screen reader is told after a line moved. */
  let announcement = $state('');

  function save(next: readonly SortOption[]): void {
    if (next === options) return;
    void board.setViewFields(view.id, { sort: [...next] }, 'sort');
  }

  const addActions = $derived<MenuAction[]>(
    sortCandidates(options, templates).map((propertyId) => ({
      id: propertyId,
      label: propertyName(propertyId, templates),
      run: () => {
        save(addSort(options, propertyId));
      },
    })),
  );

  async function move(index: number, step: -1 | 1): Promise<void> {
    const moved = options[index];
    if (!moved) return;
    save(moveSort(options, index, step));
    announcement = m.viewheader_sort_position({
      property: propertyName(moved.propertyId, templates),
      position: index + step + 1,
      count: options.length,
    });
    await tick();
    // The line took its buttons along; at an end of the list the one that was pressed is off.
    const line = root?.querySelector(`[data-sort="${CSS.escape(moved.propertyId)}"]`);
    const pressed = line?.querySelector<HTMLButtonElement>(`[data-move="${step}"]`);
    const other = line?.querySelector<HTMLButtonElement>(`[data-move="${-step}"]`);
    (pressed?.disabled ? other : pressed)?.focus();
  }

  async function remove(index: number): Promise<void> {
    save(removeSort(options, index));
    await tick();
    root?.querySelector<HTMLElement>('[data-tool="add-sort"]')?.focus();
  }
</script>

<div
  bind:this={root}
  class="flex max-w-full flex-col gap-2 p-2 group-data-[presentation=popover]/surface:w-[24rem]"
  data-testid="sort-editor"
>
  <p class="sr-only" role="status">{announcement}</p>
  {#if options.length === 0}
    <p class="px-1 text-sm text-muted-foreground">{m.viewheader_sort_none()}</p>
  {:else}
    <ol class="m-0 flex list-none flex-col gap-2 p-0" aria-label={m['ViewHeader.sort']()}>
      {#each options as option, index (option.propertyId)}
        {@const name = propertyName(option.propertyId, templates)}
        {@const direction = option.reversed
          ? m.viewheader_sort_descending()
          : m.viewheader_sort_ascending()}
        <li class="flex items-center gap-1" data-sort={option.propertyId}>
          <span class="min-w-0 flex-1 truncate px-1 text-sm font-medium roomy:text-base">
            {name}
          </span>
          <Button
            variant="outline"
            class="shrink-0 roomy:h-touch"
            aria-label={m.viewheader_sort_direction({ property: name, direction })}
            data-reversed={option.reversed}
            onclick={() => {
              save(reverseSort(options, index));
            }}
          >
            {#if option.reversed}
              <ArrowDownWideNarrowIcon data-icon="inline-start" />
            {:else}
              <ArrowUpNarrowWideIcon data-icon="inline-start" />
            {/if}
            {direction}
          </Button>
          <IconButton
            label={m.viewheader_sort_move_up({ property: name })}
            data-move="-1"
            disabled={index === 0}
            onclick={() => void move(index, -1)}
          >
            <ArrowUpIcon />
          </IconButton>
          <IconButton
            label={m.viewheader_sort_move_down({ property: name })}
            data-move="1"
            disabled={index === options.length - 1}
            onclick={() => void move(index, 1)}
          >
            <ArrowDownIcon />
          </IconButton>
          <IconButton
            label={m.viewheader_sort_remove({ property: name })}
            data-remove
            onclick={() => void remove(index)}
          >
            <XIcon />
          </IconButton>
        </li>
      {/each}
    </ol>
  {/if}
  <ActionMenu title={m.viewheader_sort_add()} actions={addActions} align="start">
    {#snippet trigger(props: Record<string, unknown>)}
      <Button
        {...props}
        variant="ghost"
        data-tool="add-sort"
        disabled={addActions.length === 0}
        class="self-start roomy:h-touch roomy:text-base"
      >
        <PlusIcon data-icon="inline-start" />
        {m.viewheader_sort_add()}
      </Button>
    {/snippet}
  </ActionMenu>
  {#if dragged && options.length > 0}
    <p class="px-1 text-xs text-muted-foreground" data-testid="sort-notice">
      {m.viewheader_sort_manual_off()}
    </p>
  {/if}
</div>
