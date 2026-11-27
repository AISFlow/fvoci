<!--
  The cards that have no value for the date the calendar is placed by, and
  so no place on the grid (a drawer opened from the
  calendar's row). The same list on a sheet of a compact page and in the
  column at the right of the calendar; the column has a button that closes it.
  A Gantt chart has the same list, with its own words (`hint`), and marks
  the cards on a loop of the relation it draws (`loops`): they have no bar
  to carry the mark.
-->
<script lang="ts">
  import XIcon from '@lucide/svelte/icons/x';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import type { PropertyUser } from '$lib/components/properties/index.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { PropertyTemplate } from '$lib/logic/wire.ts';
  import type { BoardStore } from '$lib/state/index.ts';

  import CardRow from './CardRow.svelte';
  import type { Placed } from './placed.ts';

  interface Props {
    placed: Placed;
    templates: readonly PropertyTemplate[];
    shownProperties: readonly string[];
    users: readonly PropertyUser[];
    cardHref: (cardId: string) => string;
    onOpenCard: (cardId: string) => void;
    board: BoardStore;
    /** Opens the picker of a day for a card; none where dates cannot be set. */
    onChangeDate?: (cardId: string, line: HTMLElement | undefined) => void;
    /** Given where the list has its own heading: the column. A sheet has its title. */
    onclose?: () => void;
    /** What the list says it is, for the property the view is placed by. */
    hint?: (property: string) => string;
    /** The cards on a loop. */
    loops?: ReadonlySet<string>;
  }

  let {
    placed,
    templates,
    shownProperties,
    users,
    cardHref,
    onOpenCard,
    board,
    onChangeDate,
    onclose,
    hint = (property: string) => m.calendar_undated_hint({ property }),
    loops,
  }: Props = $props();

  const uid = $props.id();
  const rows = $derived({
    placed,
    templates,
    shownProperties,
    users,
    cardHref,
    onOpenCard,
    board,
    onChangeDate,
  });
</script>

<section
  data-testid="undated-list"
  aria-labelledby={onclose ? `${uid}-title` : undefined}
  class="flex min-h-0 flex-1 flex-col"
>
  {#if onclose}
    <header class="flex shrink-0 items-center gap-2 py-1 pr-1 pl-4">
      <h3 id="{uid}-title" class="font-semibold">{m.calendar_undated()}</h3>
      <div class="flex-1"></div>
      <IconButton label={m.calendar_close_undated()} data-testid="undated-close" onclick={onclose}>
        <XIcon />
      </IconButton>
    </header>
  {/if}
  {#if placed.template && placed.undated.length > 0}
    <p class="px-4 pb-1 text-xs text-muted-foreground">
      {hint(placed.template.name)}
    </p>
  {/if}
  <ul
    class="min-h-0 flex-1 overflow-y-auto overscroll-contain px-1 pb-[max(0.5rem,var(--fab-room,0px))]"
  >
    {#each placed.undated as card (card.id)}
      <CardRow {card} {...rows} loop={loops?.has(card.id) ?? false} />
    {:else}
      <li class="px-3 py-2 text-muted-foreground">{m.calendar_no_undated()}</li>
    {/each}
  </ul>
</section>
