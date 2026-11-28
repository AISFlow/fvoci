<!--
  Everything on its way between the page and the server, in one place
  (state/transfers.svelte.ts): the archives exported and imported, and every
  file uploaded, of the workspace shown. One card each (TransferCard) with how
  far it is, until a moment after it is done; a moment that waits while the
  pointer is over the stack or the focus in it. A failed upload stays, with
  why, until it is dismissed. Closing one under way asks, then gives it up.
  More uploads than four at once are one card, with the list of each behind
  a button.

  Where it is: at the bottom right, above any dialog, which does not close
  when it is pressed, and above what is pinned to the bottom edge (the round
  "New" button of a folded board header, the comment field of an open card).
  In a window narrower than `sm` (40rem) it is as wide as the window and
  above a line of problems too, and it steps aside while the reader writes
  (the focus in a field): there the keyboard, the comment field and the line
  being written are all at the bottom. The top is the bars' buttons. The
  uploads are one card there as soon as there are two. It steps aside, too,
  while a sheet is open from the bottom (the menus and pickers of a phone),
  whose last rows it would cover.

  The keyboard reaches it by F8 from anywhere, an open dialog too (whose
  trap would take the focus back, and whose Tab goes round inside it), and
  Escape there takes the focus back to where it was: the dialog stays.
  Stepped aside, it is shown again for as long as the focus is in it. Its
  name says the key.

  Only a start, an end and a failure are announced, each a line added to one
  quiet log, so that two at once are both read: not the counts that change
  with every piece, nor what was said before the workspace was shown.
-->
<script lang="ts">
  import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
  import XIcon from '@lucide/svelte/icons/x';
  import { tick, untrack } from 'svelte';
  import type { Attachment } from 'svelte/attachments';
  import { on } from 'svelte/events';
  import { MediaQuery } from 'svelte/reactivity';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { inField } from '$lib/keys.ts';
  import type { Transfer, Workspace } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import TransferAsk from './TransferAsk.svelte';
  import TransferCard from './TransferCard.svelte';
  import { transferSaying, underWay, uploadsLabel } from './transfer-words.ts';

  let { workspace }: { workspace: Workspace } = $props();

  const uid = $props.id();
  /** More uploads than this at once are one card. */
  const GROUP_OVER = 4;
  /** At the bottom right, rather than across the top bar (Tailwind's `sm`). */
  const wide = new MediaQuery('min-width: 40rem', true);

  const shown = $derived(workspace.transfers.of(workspace.info?.id));
  const archives = $derived(shown.filter((one) => one.kind !== 'upload'));
  const uploads = $derived(shown.filter((one) => one.kind === 'upload'));
  const grouped = $derived(uploads.length > (wide.current ? GROUP_OVER : 1));
  const running = $derived(uploads.filter(underWay));
  /** Not uploaded: one kept is in already (`kept`), and is not counted with them. */
  const failed = $derived(
    uploads.filter((one) => one.failure !== undefined && one.failure !== 'kept'),
  );

  /** The transfer, or the group of uploads, the reader is asked about giving up. */
  let asking = $state<number | 'group'>();
  /** The group's list of each upload is shown. */
  let opened = $state(false);
  let stack = $state<HTMLElement>();
  /** Where the focus was when F8 brought it into the stack. */
  let cameFrom: HTMLElement | undefined;
  /** The focus is in a field: a narrow window keeps its bottom for what is written. */
  let writing = $state(false);
  /** F8 brought the focus in: shown, where an open sheet would have it aside, while it is there. */
  let reached = $state(false);

  /**
   * The last saying as the workspace `_shown` came to be shown, or the page:
   * what it says is not said again. Read again only when another is shown.
   */
  function saidBefore(_shown: string | undefined): number {
    return untrack(() => workspace.transfers.said.at(-1)?.key ?? 0);
  }
  // Of the id: the workspace's info is replaced with the same id too, which shows no other.
  const shownId = $derived(workspace.info?.id);
  const since = $derived(saidBefore(shownId));
  /** The starts, ends and failures of the workspace shown since, for a screen reader. */
  const sayings = $derived.by(() => {
    // Read first: with nothing said yet, the filter would not ask.
    const after = since;
    return workspace.transfers.said.filter(
      ({ key, transfer }) =>
        key > after && (transfer.workspaceId === undefined || transfer.workspaceId === shownId),
    );
  });

  const groupLabel = $derived(uploadsLabel(running.length, failed.length, uploads.length));
  const groupDone = $derived(running.reduce((sum, one) => sum + one.done, 0));
  const groupTotal = $derived(running.reduce((sum, one) => sum + one.total, 0));

  /**
   * Takes transfers out of sight. Where the focus was on one of them, it goes
   * to the next one's close, or back to where F8 brought it from, or into the
   * open dialog: not to the page, from where Escape would close the dialog.
   */
  function dismiss(ids: readonly number[]): void {
    const had = stack?.contains(document.activeElement) ?? false;
    for (const id of ids) workspace.transfers.dismiss(id);
    if (!had) return;
    void tick().then(() => {
      if (stack?.contains(document.activeElement)) return;
      const next = stack?.querySelector<HTMLElement>('[data-close]');
      const dialogs = document.querySelectorAll<HTMLElement>('[role="dialog"]');
      const back = next ?? (cameFrom?.isConnected ? cameFrom : [...dialogs].at(-1));
      if (!next) cameFrom = undefined;
      back?.focus();
    });
  }

  /** One under way is asked about first; an ended one goes. */
  function close(transfer: Transfer): void {
    if (underWay(transfer)) asking = transfer.id;
    else dismiss([transfer.id]);
  }

  /** Gives up what was asked about, or keeps it going, the focus back on its button. */
  function answer(stop: boolean): void {
    const asked = asking;
    asking = undefined;
    if (stop) {
      if (asked === 'group') dismiss(running.map((one) => one.id));
      else if (asked !== undefined) dismiss([asked]);
      return;
    }
    const at = asked === 'group' ? '[data-transfer-group]' : `[data-id="${String(asked)}"]`;
    void tick().then(() => {
      stack?.querySelector<HTMLElement>(`${at} [data-close]`)?.focus();
    });
  }

  function closeGroup(): void {
    if (running.length > 0) asking = 'group';
    else dismiss(uploads.map((one) => one.id));
  }

  /** F8 brings the focus into the stack, to its first button. */
  function reach(event: KeyboardEvent): void {
    if (event.key !== 'F8' || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) {
      return;
    }
    const first = stack?.querySelector<HTMLElement>('button');
    if (!first) return;
    event.preventDefault();
    const active = document.activeElement;
    if (active instanceof HTMLElement && !stack?.contains(active)) cameFrom = active;
    // Shown again first, where a narrow window had it step aside for a field, or a sheet.
    writing = false;
    reached = true;
    void tick().then(() => {
      first.focus();
    });
  }

  /**
   * The focus in the stack is the stack's: an open dialog does not take it
   * back (Bits UI's trap and its focus outside listen on the document, which a
   * focus stopped on the window does not reach), and Escape there takes it back
   * to where it was, or into the open dialog (TransferAsk answers it first):
   * it does not close the dialog.
   */
  const ownFocus: Attachment<HTMLElement> = (element) => {
    const stops = [
      on(
        window,
        'focusin',
        (event) => {
          if (element.contains(event.target as Node | null)) event.stopImmediatePropagation();
        },
        { capture: true },
      ),
      on(element, 'keydown', (event) => {
        if (event.key !== 'Escape') return;
        const dialogs = element.ownerDocument.querySelectorAll<HTMLElement>('[role="dialog"]');
        const back = cameFrom?.isConnected ? cameFrom : [...dialogs].at(-1);
        cameFrom = undefined;
        // Nothing to go back to: Escape is the page's.
        if (!back) return;
        event.stopPropagation();
        back.focus();
      }),
    ];
    return () => {
      for (const stop of stops) stop();
    };
  };

  /**
   * The finished stay while a mouse is over the stack or the focus in it (asked
   * when one would go: a focused button that goes takes no event with it), and
   * their whole moment once it leaves. A finger's touch rests nowhere.
   */
  const holding: Attachment<HTMLElement> = (element) => {
    const { transfers } = workspace;
    let over = false;
    const stopHolding = transfers.holdWhile(
      () => over || element.contains(element.ownerDocument.activeElement),
    );
    const stops = [
      // A press here is no press outside an open dialog: Bits UI lets a stopped one be.
      on(element, 'pointerdown', (event) => {
        event.stopPropagation();
      }),
      on(element, 'pointerenter', (event) => {
        over = event.pointerType === 'mouse';
      }),
      on(element, 'pointerleave', () => {
        over = false;
        transfers.restart();
      }),
      on(element, 'focusout', (event) => {
        if (!element.contains(event.relatedTarget as Node | null)) transfers.restart();
      }),
    ];
    return () => {
      for (const stop of stops) stop();
      stopHolding();
    };
  };
</script>

<svelte:window onkeydown={reach} />

<svelte:document
  onfocusin={(event: FocusEvent) => {
    writing = inField(event.target) && !stack?.contains(event.target as Node);
    // Outside the stack (a focus inside it does not come here, `ownFocus`): one that went
    // with the last of it took no focusout along.
    reached = false;
  }}
  onfocusout={() => {
    writing = false;
  }}
/>

<!-- In the page from the start: a live region that appears with its words is often not read.
     A line each, added: two said at once are both read, and the same words again are read again. -->
<div class="sr-only" role="log" data-testid="transfers-said">
  {#each sayings as one (one.key)}
    <p>{transferSaying(one.transfer)}</p>
  {/each}
</div>

{#if shown.length > 0}
  <section
    bind:this={stack}
    aria-label={m.shell_transfers_label()}
    class="pointer-events-none fixed right-[calc(0.75rem+var(--safe-right))] bottom-[calc(var(--transfers-lift)+var(--safe-bottom)+var(--keyboard))] left-[calc(0.75rem+var(--safe-left))] z-[60] data-writing:max-sm:hidden sm:right-[calc(1rem+var(--safe-right))] sm:left-auto sm:w-[22rem]"
    data-testid="transfers"
    data-transfers
    data-writing={writing ? '' : undefined}
    data-reached={reached ? '' : undefined}
    onfocusout={(event) => {
      // The window left for another (Alt+Tab): the focus comes back here, shown.
      if (!document.hasFocus()) return;
      if (!stack?.contains(event.relatedTarget as Node | null)) reached = false;
    }}
    {@attach holding}
    {@attach ownFocus}
  >
    <ul class="grid max-h-[calc(100dvh-var(--keyboard)-2rem)] gap-2 overflow-y-auto">
      {#each archives as transfer (transfer.id)}
        <TransferCard
          {transfer}
          asking={asking === transfer.id}
          onClose={() => {
            close(transfer);
          }}
          onAnswer={answer}
        />
      {/each}
      {#if grouped}
        <li
          class="pointer-events-auto grid gap-1.5 rounded-md border bg-popover p-3 text-sm text-popover-foreground shadow-md max-sm:p-2"
          data-transfer-group={uploads.length}
        >
          <div class="flex items-center gap-2">
            <p class="min-w-0 flex-1">
              {groupLabel}
              {#if running.length > 0 && failed.length > 0}
                <span class="text-destructive">
                  · {m.shell_uploads_failed({ count: failed.length })}
                </span>
              {/if}
            </p>
            <Button
              variant="ghost"
              size="sm"
              class="-my-1 roomy:min-h-touch"
              aria-expanded={opened}
              aria-controls="{uid}-each"
              onclick={() => {
                opened = !opened;
              }}
            >
              {m.shell_uploads_each()}
              <ChevronDownIcon
                class={['motion-safe:transition-transform', opened && 'rotate-180']}
                aria-hidden="true"
              />
            </Button>
            <IconButton
              label={running.length > 0
                ? m.shell_uploads_stop_all()
                : m.shell_uploads_dismiss_all()}
              class="-my-1 -mr-1 size-6"
              data-close
              onclick={closeGroup}
            >
              <XIcon />
            </IconButton>
          </div>
          {#if asking === 'group' && running.length > 0}
            <TransferAsk onAnswer={answer} />
          {:else if running.length > 0}
            <progress
              class="h-1.5 w-full accent-primary"
              max={groupTotal || undefined}
              value={groupTotal ? groupDone : undefined}
              aria-label={groupLabel}
            ></progress>
          {/if}
          {#if opened}
            <ul id="{uid}-each" class="max-h-[min(20rem,50dvh)] overflow-y-auto">
              {#each uploads as transfer (transfer.id)}
                <TransferCard
                  {transfer}
                  row
                  asking={asking === transfer.id}
                  onClose={() => {
                    close(transfer);
                  }}
                  onAnswer={answer}
                />
              {/each}
            </ul>
          {/if}
        </li>
      {:else}
        {#each uploads as transfer (transfer.id)}
          <TransferCard
            {transfer}
            asking={asking === transfer.id}
            onClose={() => {
              close(transfer);
            }}
            onAnswer={answer}
          />
        {/each}
      {/if}
    </ul>
  </section>
{/if}

<style>
  /* Clear of a line of problems (app/Problems.svelte), which spans a narrow window. */
  section {
    --transfers-lift: 4.5rem;
  }

  @media (min-width: 40rem) {
    section {
      --transfers-lift: 1rem;
    }
  }

  /* Above the round "New" button of a folded board header (ViewHeader.svelte). */
  :global(:root:has([data-testid='new-card'][data-round])) section {
    --transfers-lift: 5.5rem;
  }

  /* Above the comment field, the last row of an open card. */
  :global(:root:has([data-card-dialog])) section {
    --transfers-lift: 6.5rem;
  }

  /* Aside while a sheet is open from the bottom (BottomSheet): back when it closes, or reached. */
  :global(:root:has([data-slot='sheet-content'][data-side='bottom'])) section:not([data-reached]) {
    display: none;
  }
</style>
