<!--
  The heading of one column of the table.

  Pressing it opens the menu of the column (sorting, hiding, moving, width);
  dragging it moves the column. The cell is both: the table's column sorter
  takes a press that travels, and a press that does not is a click. That is
  why the menu is opened by the click and not, as a dropdown usually is, by
  the button going down.

  At its right edge is the handle that changes the width: dragged with a
  pointer, or focused and moved with the arrow keys.
-->
<script lang="ts">
  import ArrowDownIcon from '@lucide/svelte/icons/arrow-down';
  import ArrowUpIcon from '@lucide/svelte/icons/arrow-up';

  import type { MenuAction } from '$lib/components/menu/actions.ts';
  import ActionMenu from '$lib/components/menu/ActionMenu.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';

  import { TABLE_SETTINGS } from './columns.ts';
  import { CELL_FOCUS, PINNED, ROW_HEIGHT } from './host.ts';

  interface Props {
    id: string;
    name: string;
    /** The title column: it stays at the left edge and cannot be moved. */
    pinned?: boolean;
    sorted: 'ascending' | 'descending' | undefined;
    /** The reader may change the view: the menu, the handle and dragging are there. */
    canEdit: boolean;
    actions: readonly MenuAction[];
    /** A pointer is dragging this column. */
    dragging?: boolean;
    /** The keyboard carries this column: it is the stand-in that holds the focus. */
    carried?: boolean;
    /** The dragged column would land before this one, or after it. */
    drop?: 'before' | 'after' | undefined;
    /** The width now, in pixels, for assistive technology. */
    width: number | undefined;
    /** The handle was taken hold of. */
    onResizeStart: () => void;
    /** The handle was dragged to this width; nothing is stored yet. */
    onResizePreview: (width: number) => void;
    /** The handle was let go: store the last width, or forget it. */
    onResizeEnd: (commit: boolean) => void;
    /** An arrow key on the handle: one step wider (1) or narrower (-1). */
    onResizeStep: (direction: 1 | -1) => void;
  }

  let {
    id,
    name,
    pinned = false,
    sorted,
    canEdit,
    actions,
    dragging = false,
    carried = false,
    drop,
    width,
    onResizeStart,
    onResizePreview,
    onResizeEnd,
    onResizeStep,
  }: Props = $props();

  let open = $state(false);
  let element = $state<HTMLElement | null>(null);
  /** The drag of the handle in progress: where the pointer and the width started. */
  let resize: { pointerId: number; x: number; width: number; sign: 1 | -1 } | undefined;

  const movable = $derived(canEdit && !pinned);

  function isRtl(): boolean {
    return element !== null && getComputedStyle(element).direction === 'rtl';
  }

  function onHandleDown(event: PointerEvent): void {
    if (event.button !== 0 || !element) return;
    // The press is the handle's alone: it neither drags the column nor opens the menu.
    // (This runs in the capture phase, on the handle itself: a handler that the
    // framework delegates would run after the column sorter had seen the press.)
    event.stopPropagation();
    event.preventDefault();
    resize = {
      pointerId: event.pointerId,
      x: event.clientX,
      width: element.getBoundingClientRect().width,
      sign: isRtl() ? -1 : 1,
    };
    if (event.currentTarget instanceof HTMLElement) {
      event.currentTarget.setPointerCapture(event.pointerId);
    }
    onResizeStart();
  }

  function onHandleMove(event: PointerEvent): void {
    if (event.pointerId !== resize?.pointerId) return;
    onResizePreview(resize.width + (event.clientX - resize.x) * resize.sign);
  }

  function endResize(event: PointerEvent, commit: boolean): void {
    if (event.pointerId !== resize?.pointerId) return;
    resize = undefined;
    onResizeEnd(commit);
  }

  function onHandleKey(event: KeyboardEvent): void {
    if (event.key === 'Escape' && resize) {
      event.preventDefault();
      event.stopPropagation();
      resize = undefined;
      onResizeEnd(false);
      return;
    }
    if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return;
    // The arrows are the handle's here, not the table's.
    event.preventDefault();
    event.stopPropagation();
    onResizeStep((event.key === 'ArrowRight') === isRtl() ? -1 : 1);
  }
</script>

{#snippet cell(props: Record<string, unknown>)}
  <div
    {...props}
    bind:this={element}
    role="columnheader"
    aria-label={name}
    aria-sort={sorted}
    tabindex="-1"
    data-cell
    data-kind="header"
    data-col={id}
    data-pinned={pinned ? '' : undefined}
    data-drag-item={movable ? id : undefined}
    data-drag-scope={movable ? 'column' : undefined}
    data-drag-carried={carried ? id : undefined}
    data-dragging={dragging ? '' : undefined}
    data-drop={drop}
    class={[
      'flex min-w-0 items-center gap-1 border-e border-b bg-background ps-3 pe-2 text-start font-medium text-muted-foreground select-none',
      ROW_HEIGHT,
      CELL_FOCUS,
      pinned ? PINNED : 'relative',
      canEdit && 'cursor-pointer hover:bg-muted aria-expanded:bg-muted',
      dragging && 'opacity-50',
      carried && 'bg-muted ring-2 ring-ring ring-inset',
      drop === 'before' &&
        'before:pointer-events-none before:absolute before:inset-y-0 before:start-0 before:z-20 before:w-0.5 before:bg-ring',
      drop === 'after' &&
        'before:pointer-events-none before:absolute before:inset-y-0 before:end-0 before:z-20 before:w-0.5 before:bg-ring',
    ]}
    onpointerdown={undefined}
    onpointerup={undefined}
    onkeydown={undefined}
    onclick={() => {
      if (canEdit) open = true;
    }}
  >
    <span class="min-w-0 flex-1 truncate">{name}</span>
    {#if sorted === 'ascending'}
      <ArrowUpIcon class="size-3.5 shrink-0" aria-hidden="true" />
    {:else if sorted === 'descending'}
      <ArrowDownIcon class="size-3.5 shrink-0" aria-hidden="true" />
    {/if}
    {#if canEdit}
      <!-- svelte-ignore a11y_no_noninteractive_element_interactions (a separator that can be moved is a control: the window splitter of ARIA) -->
      <div
        role="separator"
        aria-orientation="vertical"
        aria-label={m.table_column_resize({ name })}
        aria-valuenow={width === undefined ? undefined : Math.round(width)}
        aria-valuemin={TABLE_SETTINGS.minColumnWidth}
        aria-valuemax={TABLE_SETTINGS.maxColumnWidth}
        tabindex="-1"
        data-inner
        data-resize={id}
        class="absolute inset-y-0 end-0 z-10 w-2 cursor-col-resize touch-none outline-none after:absolute after:inset-y-1 after:end-0 after:w-0.5 after:rounded-full after:bg-transparent hover:after:bg-ring focus-visible:after:bg-ring active:after:bg-ring roomy:w-3"
        onpointerdowncapture={onHandleDown}
        onpointermove={onHandleMove}
        onpointerup={(event) => {
          endResize(event, true);
        }}
        onpointercancel={(event) => {
          endResize(event, false);
        }}
        onclick={(event) => {
          event.stopPropagation();
        }}
        onkeydown={onHandleKey}
      ></div>
    {/if}
  </div>
{/snippet}

{#if canEdit}
  <ActionMenu
    bind:open
    title={m.table_column_menu({ name })}
    {actions}
    align="start"
    trigger={cell}
  />
{:else}
  {@render cell({})}
{/if}
