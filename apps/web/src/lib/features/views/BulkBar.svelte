<!--
  The bar of a selection: while cards of a view are selected it floats at the
  bottom of the view, and what is set there is set on all of them, in one
  request and one step of undo (state/board-bulk.ts).

  "N selected", then whom the cards are given to, their status (what the view
  groups by, else the board's status) and their due date, each opening the
  editor a card has for it; then the other properties ("Properties"), Delete,
  and a button that lets the selection go. A property chosen from the menu
  stays in the bar as a button of its own until the selection goes.

  Where the view is narrow the three keep only their icons, and the other
  properties and Delete go under "More": the button of a property chosen
  there is for assistive technology alone, and its editor, closed, gives the
  focus back to "More". The bar then sits above the button that adds a card
  (`--fab-room`), and above the keyboard.

  The view draws it only while something is selected and the board may be
  edited, inside an element that is `relative` and as wide as the view. The
  buttons that name a property give way where the bar is short of room, and
  on a very narrow screen the count keeps only its number: the button that
  lets the selection go stays in view. When that button or Delete takes the
  bar away, the focus goes back to a card of the view (`onLeave`).
-->
<script lang="ts">
  import { tick } from 'svelte';

  import CalendarDaysIcon from '@lucide/svelte/icons/calendar-days';
  import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
  import CircleDotIcon from '@lucide/svelte/icons/circle-dot';
  import EllipsisIcon from '@lucide/svelte/icons/ellipsis';
  import TrashIcon from '@lucide/svelte/icons/trash-2';
  import UserRoundIcon from '@lucide/svelte/icons/user-round';
  import XIcon from '@lucide/svelte/icons/x';

  import ActionMenu from '$lib/components/menu/ActionMenu.svelte';
  import type { MenuAction } from '$lib/components/menu/actions.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { overlayPresentation } from '$lib/layout/presentation.ts';
  import { page, pane } from '$lib/layout/room.svelte.ts';
  import { statusPropertyOf } from '$lib/logic/card-hierarchy.ts';
  import type { BoardView, PropertyTemplate, PropertyValue } from '$lib/logic/wire.ts';
  import { CARD_BATCH_LIMIT, setCardsProperty, type BoardStore } from '$lib/state/index.ts';
  import { cn } from '$lib/utils.js';

  import { bulkProperties, cardAfter, sharedValue } from './bulk-bar.ts';
  import BulkValueEditor from './BulkValueEditor.svelte';
  import type { Selection } from './selection.svelte.ts';

  interface Props {
    board: BoardStore;
    view: BoardView;
    selection: Selection;
    /**
     * Puts the focus, which was on the bar, back in the view once ✕ or Delete
     * took the bar away: on the first of `cardIds` still there, unless they
     * were `deleted`. Without it, on that card's element (`[data-card]`).
     */
    onLeave?: (cardIds: readonly string[], deleted: boolean) => void;
  }

  let { board, view, selection, onLeave }: Props = $props();

  interface Role {
    template: PropertyTemplate;
    icon: typeof UserRoundIcon;
  }

  const templates = $derived(board.board?.cardProperties ?? []);
  const roles = $derived(
    bulkProperties(templates, {
      groupBy: board.groupingOf(view)?.id,
      status: statusPropertyOf(templates)?.id,
      due: board.board?.dueDatePropertyId,
    }),
  );
  const shown = $derived.by((): Role[] => {
    const { assignee, status, due } = roles;
    return [
      ...(assignee ? [{ template: assignee, icon: UserRoundIcon }] : []),
      ...(status ? [{ template: status, icon: CircleDotIcon }] : []),
      ...(due ? [{ template: due, icon: CalendarDaysIcon }] : []),
    ];
  });
  const cards = $derived(
    selection.ids.flatMap((id) => {
      const card = board.cardsById.get(id);
      return card ? [card] : [];
    }),
  );
  const workspace = $derived(board.workspace);
  const presentation = $derived(overlayPresentation(page.stage));
  const wide = $derived(pane.row);

  /** The property chosen from the menu, and how many times one was: each choice opens it anew. */
  let chosenId = $state<string | undefined>(undefined);
  let choices = $state(0);
  const chosen = $derived(roles.others.find((template) => template.id === chosenId));

  function put(template: PropertyTemplate, value: PropertyValue | undefined): void {
    void setCardsProperty(board, selection.ids, template.id, value);
  }

  /** The element of the bar, whose parent is the view's. */
  let wrapper = $state<HTMLElement>();

  /** The button of the bar's menu, where the focus goes from an editor whose opener is not seen. */
  function menuButton(): HTMLElement | null | undefined {
    return wrapper?.querySelector<HTMLElement>('[data-bulk-menu]');
  }

  /**
   * Lets the selection go, by ✕ or Delete. Once the bar is gone, a focus it
   * had (on the body now) goes back to the view; one elsewhere stays.
   */
  function leave(deleted: boolean): void {
    const ids = selection.ids;
    const root = wrapper?.parentElement;
    selection.clear();
    if (deleted) void board.deleteCards(ids);
    void tick().then(() => {
      const active = document.activeElement;
      if (active && active !== document.body && active.isConnected) return;
      if (onLeave) onLeave(ids, deleted);
      else if (root)
        cardAfter([...root.querySelectorAll<HTMLElement>('[data-card]')], ids, deleted)?.focus();
    });
  }

  function remove(): void {
    leave(true);
  }

  const deleteAction = $derived<MenuAction>({
    id: 'delete',
    label: m.bulk_delete(),
    icon: TrashIcon,
    hint: String(selection.ids.length),
    destructive: true,
    separated: roles.others.length > 0,
    run: remove,
  });

  const menuActions = $derived<MenuAction[]>([
    ...roles.others.map((template): MenuAction => ({
      id: `property-${template.id}`,
      label: template.name,
      hint: sharedValue(cards, template.id).mixed ? m.bulk_mixed() : undefined,
      afterClose: () => {
        chosenId = template.id;
        choices += 1;
      },
    })),
    ...(wide ? [] : [deleteAction]),
  ]);

  /**
   * The buttons of the bar: 36 pixels high, 44 where a finger presses. A
   * button with an icon alone keeps its size; one with words gives way, down
   * to its icon, where the bar is short of room.
   */
  const BUTTON =
    'inline-flex items-center justify-center rounded-lg text-sm font-medium outline-none select-none hover:bg-muted focus-visible:ring-3 focus-visible:ring-ring/50 aria-expanded:bg-muted';
  const iconButton = cn(BUTTON, 'size-9 shrink-0 roomy:size-touch');
  const textButton = cn(
    BUTTON,
    'h-9 min-w-0 shrink gap-1.5 overflow-hidden px-2 whitespace-nowrap roomy:h-touch',
  );
</script>

{#snippet roleEditor(
  template: PropertyTemplate,
  Icon: Role['icon'] | undefined,
  startOpen: boolean,
)}
  {@const shared = sharedValue(cards, template.id)}
  {@const label = shared.mixed ? m.bulk_value_mixed({ name: template.name }) : template.name}
  {#snippet opener({ props }: { props: Record<string, unknown> })}
    {#if wide || !Icon}
      <button
        {...props}
        type="button"
        title={label}
        data-bulk-property={template.id}
        data-mixed={shared.mixed ? '' : undefined}
        tabindex={wide ? undefined : -1}
        class={cn(textButton, !wide && 'sr-only')}
      >
        {#if Icon}<Icon class="size-4 shrink-0" />{/if}
        <span class="max-w-32 min-w-0 truncate">{template.name}</span>
        {#if shared.mixed}
          <span class="shrink-0 text-muted-foreground">{m.bulk_mixed()}</span>
        {/if}
      </button>
    {:else}
      <button
        {...props}
        type="button"
        aria-label={label}
        title={label}
        data-bulk-property={template.id}
        data-mixed={shared.mixed ? '' : undefined}
        class={cn(iconButton, 'relative')}
      >
        <Icon class="size-4" />
        {#if shared.mixed}
          <!-- The values differ: the picker says so in its heading. -->
          <span
            aria-hidden="true"
            data-slot="bulk-mixed"
            class="absolute top-1.5 right-1.5 size-2 rounded-full bg-primary ring-2 ring-popover roomy:top-2.5 roomy:right-2.5"
          ></span>
        {/if}
      </button>
    {/if}
  {/snippet}
  <BulkValueEditor
    {template}
    {shared}
    users={board.people}
    named={workspace.users}
    locale={workspace.locale}
    regional={workspace.regional}
    {presentation}
    {startOpen}
    {opener}
    returnFocus={() => (wide || Icon ? undefined : menuButton())}
    onChange={(value: PropertyValue | undefined) => {
      put(template, value);
    }}
  />
{/snippet}

<!--
  Above the keyboard or the home indicator, whichever is higher. Where the
  add button is there (`--fab-room`), above it: that button counts both, so
  the smaller of the two is added back (the `min`, which is 0 without it).
-->
<div
  bind:this={wrapper}
  class="@container/bulk pointer-events-none absolute inset-x-0 bottom-[calc(max(var(--safe-bottom),var(--keyboard,0px))+min(var(--safe-bottom),var(--keyboard,0px),var(--fab-room,0px))+var(--fab-room,0px)+0.5rem)] z-30 flex flex-col items-center gap-1 px-2"
>
  {#if selection.full}
    <p
      role="status"
      class="pointer-events-auto rounded-md bg-foreground px-2 py-1 text-xs text-background shadow"
    >
      {m.bulk_full({ limit: CARD_BATCH_LIMIT })}
    </p>
  {/if}
  <div
    role="group"
    aria-label={m.bulk_actions()}
    data-testid="selection-bar"
    class="pointer-events-auto flex max-w-full min-w-0 items-center gap-0.5 rounded-xl border bg-popover p-1 text-popover-foreground shadow-lg"
  >
    <!-- Narrower than 360 pixels the count shows its number alone; it still says it whole. -->
    <span
      role="status"
      data-slot="bulk-count"
      class="shrink-0 px-2 text-sm font-medium whitespace-nowrap tabular-nums @max-[22.5rem]/bulk:px-1.5"
    >
      <span class="@max-[22.5rem]/bulk:sr-only">
        {m.bulk_selected_count({ count: selection.ids.length })}
      </span>
      <span aria-hidden="true" class="hidden @max-[22.5rem]/bulk:inline">
        {selection.ids.length}
      </span>
    </span>
    {#each shown as role (role.template.id)}
      {@render roleEditor(role.template, role.icon, false)}
    {/each}
    {#if chosen}
      {#key choices}
        {@render roleEditor(chosen, undefined, true)}
      {/key}
    {/if}
    {#if menuActions.length > 0}
      <ActionMenu
        title={wide ? m.bulk_properties() : m.bulk_more()}
        actions={menuActions}
        align="start"
      >
        {#snippet trigger(props: Record<string, unknown>)}
          {#if wide}
            <button {...props} type="button" data-bulk-menu class={textButton}>
              <span class="min-w-0 truncate">{m.bulk_properties()}</span>
              <ChevronDownIcon class="size-4 shrink-0" />
            </button>
          {:else}
            <button
              {...props}
              type="button"
              aria-label={m.bulk_more()}
              title={m.bulk_more()}
              data-bulk-menu
              class={iconButton}
            >
              <EllipsisIcon class="size-4" />
            </button>
          {/if}
        {/snippet}
      </ActionMenu>
    {/if}
    {#if wide}
      <button
        type="button"
        data-bulk-delete
        class={cn(textButton, 'text-destructive hover:text-destructive')}
        onclick={remove}
      >
        <TrashIcon class="size-4 shrink-0" />
        <span class="min-w-0 truncate">{m.bulk_delete()}</span>
      </button>
    {/if}
    <button
      type="button"
      aria-label={m.bulk_clear()}
      title={m.bulk_clear()}
      data-bulk-clear
      class={iconButton}
      onclick={() => {
        leave(false);
      }}
    >
      <XIcon class="size-4" />
    </button>
  </div>
</div>
