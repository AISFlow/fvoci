<!--
  What was done to a card, the newest first: its making, its title, the
  values of its properties, its parent, whether it is a template, its coming
  back from the trash. Folded at first: read when it is opened, a page at a
  time; while it is open, a change of the card (`blockChanged`, or the
  reader's own) reads the first page again.

  Each line is two: who and when, then what. A person is named as the reader
  knows them; one the reader may not see (a guest sees only some of the
  workspace) or whose account is gone is shown without a name. What an agent
  did carries the name of the token it did it through.
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import { displayDateTime, relativeTime, type DisplayContext } from '$lib/logic/format.ts';
  import {
    ACTIVITY_PAGE,
    type BoardStore,
    type CardActivityLine,
    type CardSummary,
  } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import { activityText } from './activity-text.ts';

  interface Props {
    board: BoardStore;
    card: CardSummary;
  }

  let { board, card }: Props = $props();

  const uid = $props.id();
  const workspace = $derived(board.workspace);

  let open = $state(false);
  let lines = $state.raw<CardActivityLine[]>([]);
  /** The last page was full: there may be more. */
  let more = $state(false);
  let reading = $state(false);
  let failed = $state(false);
  /** When the lines were last read: the times are said from it. */
  let readAt = $state(Date.now());
  let pages = 0;

  async function read(page: number): Promise<CardActivityLine[] | undefined> {
    const activity = workspace.backend.cardActivity;
    if (!activity) return undefined;
    try {
      const found = await activity.list(board.boardId, card.id, { page, perPage: ACTIVITY_PAGE });
      const people = found.flatMap((line) => (line.actorId ? [line.actorId] : []));
      void workspace.learnUsers([...new Set(people)]);
      failed = false;
      readAt = Date.now();
      return found;
    } catch {
      failed = true;
      return undefined;
    }
  }

  async function readOn(): Promise<void> {
    if (reading) return;
    reading = true;
    const askedAt = card.updatedAt;
    const found = await read(pages);
    reading = false;
    if (!found) return;
    pages += 1;
    const known = new Set(lines.map((line) => line.id));
    lines = [...lines, ...found.filter((line) => !known.has(line.id))];
    more = found.length === ACTIVITY_PAGE;
    // A change that came while the first page was read is read again.
    if (pages === 1 && card.updatedAt !== askedAt) void readFirst(card.updatedAt);
  }

  /** The change of the card the first page was last asked to be read anew for. */
  let changeAsked = 0;

  /**
   * The first page anew, for the card as it was changed at `changedAt`: its
   * lines in place of those it has, and before the older ones. A reading for
   * an older change that comes back after a newer one is dropped.
   */
  async function readFirst(changedAt: number): Promise<void> {
    changeAsked = changedAt;
    const found = await read(0);
    if (!found || changedAt !== changeAsked) return;
    const fresh = new Set(found.map((line) => line.id));
    const oldest = found.at(-1)?.updatedAt ?? Infinity;
    const older = lines.filter((line) => !fresh.has(line.id) && line.updatedAt <= oldest);
    lines = found.length < ACTIVITY_PAGE ? found : [...found, ...older];
    if (found.length < ACTIVITY_PAGE) [pages, more] = [1, false];
  }

  // Opened, or the card changed while it is open.
  $effect(() => {
    if (!open) return;
    const changedAt = card.updatedAt;
    untrack(() => {
      void (pages === 0 ? readOn() : readFirst(changedAt));
    });
  });

  const context = $derived<DisplayContext>({
    locale: workspace.locale,
    timeZone: workspace.regional.timeZone,
    hourCycle: workspace.regional.hourCycle,
    cardTitle: (cardId: string) => board.card(cardId)?.title,
    personName: (userId: string) => workspace.userName(userId) ?? m['centerPanel.unknown-user'](),
  });
  const templates = $derived(board.board?.cardProperties ?? []);

  const nameOf = (line: CardActivityLine): string =>
    (line.actorId && workspace.userName(line.actorId)) || m['centerPanel.unknown-user']();
</script>

<section
  aria-labelledby="{uid}-heading"
  class="flex flex-col gap-3 border-t pt-3"
  data-testid="card-activity"
>
  <h3 id="{uid}-heading" class="font-semibold">
    <Button
      variant="ghost"
      class="-ml-2 min-h-touch gap-1.5 px-2 text-base font-semibold"
      aria-expanded={open}
      aria-controls="{uid}-lines"
      onclick={() => {
        open = !open;
      }}
    >
      <ChevronRightIcon
        class={['size-4 transition-transform', open && 'rotate-90']}
        aria-hidden="true"
      />
      {m.card_activity()}
    </Button>
  </h3>
  {#if open}
    <div id="{uid}-lines" class="flex flex-col gap-3">
      {#if failed && lines.length === 0}
        <p class="text-sm text-muted-foreground" role="alert">{m.app_problem_load()}</p>
      {:else if lines.length === 0 && !reading}
        <p class="text-sm text-muted-foreground">{m.card_activity_none()}</p>
      {/if}
      <ol class="flex flex-col gap-3">
        {#each lines as line (line.id)}
          <li class="flex min-w-0 flex-col gap-0.5 text-sm" data-kind={line.kind}>
            <p class="flex min-w-0 items-center gap-x-2 text-muted-foreground">
              <span class="min-w-0 truncate font-medium text-foreground">{nameOf(line)}</span>
              {#if line.actorTokenName}
                <span
                  class="min-w-0 truncate rounded-sm bg-muted px-1 text-xs font-medium"
                  data-slot="agent-badge"
                >
                  {m.agent_badge({ name: line.actorTokenName })}
                </span>
              {/if}
              <time
                class="shrink-0"
                datetime={new Date(line.updatedAt).toISOString()}
                title={displayDateTime(line.updatedAt, context)}
              >
                {relativeTime(line.updatedAt, workspace.locale, Math.max(readAt, line.updatedAt))}
              </time>
            </p>
            <p class="min-w-0 break-words">{activityText(line, { card, templates, context })}</p>
          </li>
        {/each}
      </ol>
      {#if more}
        <Button
          variant="ghost"
          class="self-start roomy:min-h-touch"
          disabled={reading}
          onclick={() => void readOn()}
        >
          {m.card_activity_more()}
        </Button>
      {/if}
    </div>
  {/if}
</section>
