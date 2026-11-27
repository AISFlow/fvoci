<!--
  What stands after the last column: "+ Add a group", and the columns the
  view hides, each with its name, how many cards it holds and "Show".

  A hidden column is a place to drop cards on (lib/drag, a zone): they take
  its value and leave the columns that are shown. There is no place within
  it. While the keyboard carries a card over one, the card is drawn here and
  holds the focus.
-->
<script lang="ts">
  import CardFace from '$lib/components/card-face/CardFace.svelte';
  import OptionChip from '$lib/components/option-chip/OptionChip.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { BoardGroup } from '$lib/logic/group.ts';
  import type { Card } from '$lib/logic/wire.ts';
  import { Button } from '$lib/ui/button/index.js';

  import { badgesOf, type FaceContext } from './board-data.ts';
  import { hiddenIdOf } from './kanban.ts';

  interface Props {
    panelId: string;
    hidden: readonly BoardGroup[];
    nameOf: (group: BoardGroup) => string;
    /** The columns are options with colours of their own. */
    isOption: boolean;
    /** The reader may change the view. */
    editable: boolean;
    /** A group can be added: the board is grouped by a select property. */
    canAdd: boolean;
    face: FaceContext;
    /** The hidden column the dragged cards are over, by its option. */
    overOptionId: string | undefined;
    /** The card the keyboard carries, while it is over a hidden column. */
    carried: Card | undefined;
    carriedCount: number;
    cardHelpId: string;
    onShow: (optionId: string) => void;
    onAddGroup: () => void;
  }

  let {
    panelId,
    hidden,
    nameOf,
    isOption,
    editable,
    canAdd,
    face,
    overOptionId,
    carried,
    carriedCount,
    cardHelpId,
    onShow,
    onAddGroup,
  }: Props = $props();
</script>

<aside
  id={panelId}
  data-panel
  data-testid="kanban-more"
  aria-label={hidden.length > 0 ? m['BoardComponent.hidden-columns']() : m.shared_more()}
  class="flex w-full max-w-sm shrink-0 snap-center flex-col gap-1 @columns/pane:w-64 @columns/pane:max-w-none @columns/pane:snap-align-none @header/pane:w-68"
>
  {#if canAdd}
    <Button
      variant="ghost"
      data-testid="add-group"
      class="justify-start text-muted-foreground roomy:min-h-touch"
      onclick={onAddGroup}
    >
      {m['BoardComponent.add-a-group']()}
    </Button>
  {/if}
  {#if hidden.length > 0}
    <h3 class="px-2.5 pt-2 pb-1 text-xs font-medium text-muted-foreground">
      {m['BoardComponent.hidden-columns']()}
    </h3>
    <ul class="flex flex-col gap-1" data-testid="hidden-columns">
      {#each hidden as group (group.option.id)}
        {@const name = nameOf(group)}
        {@const over = overOptionId === group.option.id}
        <li
          data-drag-zone={hiddenIdOf(group.option.id)}
          data-hidden-column={group.option.id}
          data-over={over ? '' : undefined}
          class={[
            'flex flex-col gap-2 rounded-lg border-2 border-dashed p-1 pl-2',
            over ? 'border-primary bg-accent' : 'border-transparent bg-muted',
          ]}
        >
          <div class="flex min-h-8 items-center gap-2 roomy:min-h-touch">
            <span class="flex min-w-0 flex-1 items-center gap-2">
              {#if isOption && group.option.id !== ''}
                <OptionChip {name} color={group.option.color} />
              {:else}
                <span class="truncate text-sm font-medium" title={name}>{name}</span>
              {/if}
              <span class="shrink-0 text-xs text-muted-foreground" data-hidden-count>
                <span aria-hidden="true">{group.cards.length}</span>
                <span class="sr-only">{m.kanban_card_count({ count: group.cards.length })}</span>
              </span>
            </span>
            {#if editable}
              <Button
                variant="ghost"
                size="sm"
                class="shrink-0 roomy:min-h-touch"
                aria-label={m.kanban_show_column({ name })}
                onclick={() => {
                  onShow(group.option.id);
                }}
              >
                {m['BoardComponent.show']()}
              </Button>
            {/if}
          </div>
          {#if over && carried}
            {@const badges = badgesOf(carried)}
            <div
              data-drag-carried={carried.id}
              tabindex="-1"
              aria-describedby={cardHelpId}
              class="rounded-md outline-none select-none"
            >
              <CardFace
                card={carried}
                templates={face.templates}
                shownProperties={face.shownProperties}
                hasBody={badges.hasBody}
                tasksTotal={badges.tasksTotal}
                tasksDone={badges.tasksDone}
                locale={face.locale}
                regional={face.regional}
                users={face.users}
                lifted
                count={carriedCount}
              />
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</aside>
