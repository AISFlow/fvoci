<!--
  Who is on the board now and how many members it has, at the head of the board.

  The faces of the others here now (five, two in a narrow pane, then "+k" for the
  rest and for those the reader may not see; in a narrow pane also for the anonymous
  readers of its public link) open the list of who is here and
  which cards they have open; with nobody else here there is no button for them. Beside them the number of the board's members opens
  the members of the sharing dialog (`onMembers`); it is there also when nobody
  else is. A narrow pane (a phone) keeps one of the two, so the board's title keeps
  its room: alone, the members as a figure and the number; with others, their faces,
  whose list ends in the members. A screen reader hears both numbers together on the group.
-->
<script lang="ts">
  import UsersIcon from '@lucide/svelte/icons/users';

  import Avatar from '$lib/components/avatar/Avatar.svelte';
  import { PickerPresentation } from '$lib/components/picker-surface/default-presentation.svelte.ts';
  import PickerSurface from '$lib/components/picker-surface/PickerSurface.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { pane } from '$lib/layout/room.svelte.ts';
  import { boardPresence, type BoardStore } from '$lib/state/index.ts';

  let { board, onMembers }: { board: BoardStore; onMembers: () => void } = $props();
  const presence = $derived(boardPresence(board));
  const workspace = $derived(board.workspace);
  const presentation = new PickerPresentation();
  const label = $derived(m.presence_viewers({ count: presence.count }));
  /** Everybody the members' list shows: each member is somebody the reader may see. */
  const members = $derived(board.orderedMembers.length);
  const membersLabel = $derived(m.presence_members_short({ count: members }));
  let listOpen = $state(false);
  /** How many faces are shown before the rest become a number; fewer in a narrow pane. */
  const FACES = 5;
  const NARROW_FACES = 2;
  const faces = $derived(presence.others.slice(0, FACES));
  /** The people not shown as faces: past those shown, and those the reader may not see. */
  const moreThan = (shown: number): number =>
    Math.max(presence.others.length - shown, 0) + presence.snapshot.hiddenCount;
  const more = $derived(moreThan(FACES));
  const narrowMore = $derived(moreThan(NARROW_FACES) + presence.snapshot.anonymousCount);
  /**
   * A narrow pane, where the faces stand in for the members' button. Read by script for the
   * list: it opens in a popover or a sheet outside the pane, where no container query reaches.
   */
  const narrow = $derived(pane.measured && !pane.columns);

  /**
   * The list closed after the last one left: its button is away now, so the focus it would get
   * back goes to the members' button instead. After the surface has put it back.
   */
  function toMembers(): void {
    requestAnimationFrame(() => {
      const group = document.querySelector('[data-testid="board-people"]');
      const faces = group?.querySelector('[data-testid="board-presence"]');
      const active = document.activeElement;
      if (active !== null && active !== document.body && active !== faces) return;
      group?.querySelector<HTMLElement>('[data-testid="board-members-count"]')?.focus();
    });
  }
</script>

<div
  role="group"
  aria-label={m.presence_and_members({ count: presence.count, members })}
  class="flex shrink-0 items-center gap-1"
  data-testid="board-people"
>
  <PickerSurface
    bind:open={
      () => listOpen,
      (open: boolean) => {
        listOpen = open;
        if (!open && presence.count === 0) toMembers();
      }
    }
    presentation={presentation.current}
    title={label}
  >
    {#snippet trigger({ props })}
      <button
        {...props}
        type="button"
        data-testid="board-presence"
        aria-label={label}
        class={[
          'flex min-h-8 shrink-0 items-center justify-center gap-1 rounded-md px-1 text-xs hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring roomy:min-h-touch roomy:min-w-touch',
          // Nobody else here, there is no face to show: the button is away, but for while
          // its list is open (it stays where it is when the last one left); the group says it.
          presence.count === 0 && !listOpen && 'hidden',
        ]}
      >
        <!-- Overlapping margins on each item instead of Tailwind space utilities, whose
             `:where(... > :not(:last-child))` triggers full-document sibling invalidation in WebKit
             whenever a portal mounts or unmounts on body. -->
        <span class="flex">
          {#each faces as person, index (person.userId)}
            <Avatar
              name={workspace.userName(person.userId) ?? m['centerPanel.unknown-user']()}
              colorKey={person.userId}
              locale={workspace.locale}
              decorative
              class={[
                'size-5 ring-2 ring-background @columns/pane:size-6',
                index > 0 && '-ms-1',
                index >= NARROW_FACES && '@max-columns/pane:hidden',
              ]}
            />
          {/each}
        </span>
        {#if narrowMore > 0}<span class="@columns/pane:hidden">+{narrowMore}</span>{/if}
        {#if more > 0}<span class="hidden @columns/pane:inline">+{more}</span>{/if}
        {#if presence.snapshot.anonymousCount > 0}
          <span class="hidden @columns/pane:inline">
            {m.presence_anonymous({ count: presence.snapshot.anonymousCount })}
          </span>
        {/if}
      </button>
    {/snippet}
    <ul class="flex max-w-80 flex-col gap-2 p-2" data-testid="presence-list">
      {#each presence.others as person (person.userId)}
        <li class="flex items-start gap-2" data-presence-user={person.userId}>
          <Avatar
            name={workspace.userName(person.userId) ?? m['centerPanel.unknown-user']()}
            colorKey={person.userId}
            locale={workspace.locale}
            decorative
          />
          <div class="min-w-0">
            <p class="break-words">
              {workspace.userName(person.userId) ?? m['centerPanel.unknown-user']()}
            </p>
            {#each person.cardIds as cardId (cardId)}
              <p class="truncate text-xs text-muted-foreground">
                {board.card(cardId)?.title || m['KanbanCard.untitled']()}
              </p>
            {/each}
          </div>
        </li>
      {/each}
      {#if presence.snapshot.anonymousCount > 0}
        <li>{m.presence_anonymous({ count: presence.snapshot.anonymousCount })}</li>
      {/if}
      {#if presence.snapshot.hiddenCount > 0}
        <li>{m.presence_hidden({ count: presence.snapshot.hiddenCount })}</li>
      {/if}
      {#if presence.count === 0}<li class="text-muted-foreground">{m.presence_alone()}</li>{/if}
      <!-- Narrow, where the members' own button gives way to the faces. -->
      {#if narrow}
        <li class="border-t pt-2">
          <button
            type="button"
            data-testid="presence-members"
            aria-haspopup="dialog"
            class="flex min-h-8 w-full items-center gap-2 rounded-md px-1 text-left hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring roomy:min-h-touch"
            onclick={() => {
              listOpen = false;
              onMembers();
            }}
          >
            <UsersIcon class="size-4" aria-hidden="true" />
            {membersLabel}
          </button>
        </li>
      {/if}
    </ul>
  </PickerSurface>
  <button
    type="button"
    data-testid="board-members-count"
    aria-haspopup="dialog"
    aria-label={membersLabel}
    title={membersLabel}
    class={[
      'flex min-h-8 shrink-0 items-center justify-center gap-1 rounded-md px-1 text-xs whitespace-nowrap text-muted-foreground hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring roomy:min-h-touch roomy:min-w-touch',
      presence.count > 0 && '@max-columns/pane:hidden',
    ]}
    onclick={onMembers}
  >
    <UsersIcon class="size-4 @columns/pane:hidden" aria-hidden="true" />
    <span class="@columns/pane:hidden" aria-hidden="true">{members}</span>
    <span class="hidden @columns/pane:inline" aria-hidden="true">{membersLabel}</span>
  </button>
</div>
