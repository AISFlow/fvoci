<!--
  Which properties a view shows on its cards: one switch per property of the
  board. A gallery can also hide the titles, and every view but a table can
  show the marks of comments and description.

  Each switch is saved as it is flipped. The list stays open: a reader
  usually flips several. Who may change the board's properties can move one
  up or down: that is the board's order, which the cards of a board and a
  gallery and the card's own page follow (a table has its columns' order).
  The focus stays with the moved property, and its new place is told.
-->
<script lang="ts">
  import ArrowDownIcon from '@lucide/svelte/icons/arrow-down';
  import ArrowUpIcon from '@lucide/svelte/icons/arrow-up';
  import { tick } from 'svelte';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { doneChoices as doneChoicesOf, doneChoiceValue } from '$lib/logic/done-rule.ts';
  import type { BoardView } from '$lib/logic/wire.ts';
  import type { BoardStore } from '$lib/state/index.ts';
  import { Switch } from '$lib/ui/switch/index.js';

  import { propertyToggles, withPropertyVisible, type PropertyToggle } from './view-fields.ts';

  let { board, view }: { board: BoardStore; view: BoardView } = $props();

  const toggles = $derived(
    propertyToggles(board.board?.cardProperties ?? [], view.fields.viewType),
  );
  /** The properties the panel lists, in the board's order: a move counts among these. */
  const order = $derived(toggles.map((toggle) => toggle.id));

  let list = $state<HTMLElement>();
  /** The property moved last, for a screen reader: where it is now. */
  let announced = $state<string | undefined>();
  const told = $derived.by(() => {
    const at = announced === undefined ? -1 : order.indexOf(announced);
    const name = toggles[at]?.template.name;
    return name === undefined
      ? ''
      : m.view_property_moved({ name, place: at + 1, count: order.length });
  });
  /** A move whose row is to keep the focus once it shows: not state, read once. */
  let pending: { id: string; step: -1 | 1; from: number } | undefined;

  function move(propertyId: string, step: -1 | 1): void {
    announced = propertyId;
    pending = { id: propertyId, step, from: order.indexOf(propertyId) };
    void board.moveProperty(propertyId, step, order);
  }

  // Once the move shows: the moved row's button keeps the focus, or the other one at an end.
  $effect(() => {
    // Read whether or not a move waits, so that the effect runs again when the order changes.
    const shown = order;
    const at = pending ? shown.indexOf(pending.id) : -1;
    if (!pending || at < 0 || at === pending.from) return;
    const { id, step } = pending;
    pending = undefined;
    void tick().then(() => {
      const row = list?.querySelector(`[data-row="${CSS.escape(id)}"]`);
      const buttons = row?.querySelectorAll<HTMLButtonElement>('button[data-action^="move-"]');
      const [up, down] = buttons ?? [];
      const wanted = step === -1 ? up : down;
      (wanted?.disabled ? (step === -1 ? down : up) : wanted)?.focus();
    });
  });

  function nameOf(toggle: PropertyToggle): string {
    return toggle.template.name;
  }

  function show(propertyId: string, visible: boolean): void {
    void board.setViewFields(
      view.id,
      {
        shownProperties: withPropertyVisible(view.fields.shownProperties, propertyId, visible),
      },
      'showProperty',
    );
  }

  const dateProperties = $derived(
    (board.board?.cardProperties ?? []).filter((prop) => prop.type === 'date'),
  );
  const doneChoices = $derived(doneChoicesOf(board.board?.cardProperties ?? []));
  const selectedDoneValue = $derived(doneChoiceValue(board.board?.doneWhen));

  function changeDueProperty(value: string): void {
    void board.changeBoard((current) => {
      const next = { ...current };
      if (value) next.dueDatePropertyId = value;
      else delete next.dueDatePropertyId;
      return next;
    }, 'changeBoard');
  }

  function changeDoneRule(value: string): void {
    const chosen = doneChoices.find((item) => item.value === value);
    void board.changeBoard((current) => {
      const next = { ...current };
      if (chosen) next.doneWhen = chosen.rule;
      else delete next.doneWhen;
      return next;
    }, 'changeBoard');
  }
</script>

<p class="sr-only" aria-live="polite">{told}</p>
<ul bind:this={list} class="m-0 flex list-none flex-col p-1" data-testid="properties-panel">
  {#each toggles as toggle (toggle.id)}
    {@const name = nameOf(toggle)}
    <li class="flex items-center gap-1" data-row={toggle.id}>
      <label
        class="flex min-h-8 min-w-0 flex-1 cursor-default items-center justify-between gap-4 rounded-md px-2 text-sm hover:bg-muted roomy:min-h-touch roomy:text-base"
      >
        <span class="min-w-0 truncate">{name}</span>
        <Switch
          aria-label={name}
          data-property={toggle.id}
          bind:checked={
            () => view.fields.shownProperties.includes(toggle.id),
            (visible: boolean) => {
              show(toggle.id, visible);
            }
          }
        />
      </label>
      {#if board.canManageProperties}
        {@const place = order.indexOf(toggle.id)}
        <IconButton
          label={m.view_property_move_up({ name })}
          class="size-7 text-muted-foreground"
          data-action="move-up"
          disabled={place === 0}
          onclick={() => {
            move(toggle.id, -1);
          }}
        >
          <ArrowUpIcon aria-hidden="true" />
        </IconButton>
        <IconButton
          label={m.view_property_move_down({ name })}
          class="size-7 text-muted-foreground"
          data-action="move-down"
          disabled={place === order.length - 1}
          onclick={() => {
            move(toggle.id, 1);
          }}
        >
          <ArrowDownIcon aria-hidden="true" />
        </IconButton>
      {/if}
    </li>
  {/each}
</ul>
{#if board.canManageProperties && board.board && !board.board.isTemplate && (dateProperties.length > 0 || doneChoices.length > 0)}
  <div class="grid gap-2 border-t p-2 text-xs" data-testid="board-today-rules">
    {#if dateProperties.length > 0}
      <label class="grid gap-1">
        <span class="font-medium text-muted-foreground">{m.view_due_date_property()}</span>
        <select
          class="h-8 rounded-md border border-input-border bg-background px-2 text-base md:text-xs"
          data-testid="board-due-date-property"
          value={board.board.dueDatePropertyId ?? ''}
          onchange={(e) => {
            changeDueProperty(e.currentTarget.value);
          }}
        >
          <option value="">{m.view_due_date_auto()}</option>
          {#each dateProperties as prop (prop.id)}
            <option value={prop.id}>{prop.name}</option>
          {/each}
        </select>
      </label>
    {/if}
    {#if doneChoices.length > 0}
      <label class="grid gap-1">
        <span class="font-medium text-muted-foreground">{m.view_done_when()}</span>
        <select
          class="h-8 rounded-md border border-input-border bg-background px-2 text-base md:text-xs"
          data-testid="board-done-when"
          value={selectedDoneValue}
          onchange={(e) => {
            changeDoneRule(e.currentTarget.value);
          }}
        >
          <option value="">{m.view_done_none()}</option>
          {#each doneChoices as choice (choice.value)}
            <option value={choice.value}>{choice.label}</option>
          {/each}
        </select>
      </label>
    {/if}
  </div>
{/if}
