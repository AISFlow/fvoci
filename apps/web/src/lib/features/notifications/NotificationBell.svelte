<!--
  The bell of the top bar: how many notifications the reader has not read,
  and, opened, the newest of them, the unread marked. One is a link to its
  card or page: a plain click goes there (`onGo`, which marks it read), and
  ⌘/Ctrl or a middle click opens it in a new tab as any link does. "Mark all
  as read" marks those shown and what came before them.

  The count and the list come from the inbox (`NotificationInbox`), which
  keeps them up to date while the page is open; the list is read when the
  bell is opened.

  Each says under it whose agent did it, where an agent did (its token's
  name), and where it happened: the board, wiki or course, and the
  organization (`sourceOf`). In a narrow pane (a phone) the bell opens the
  same list as a sheet of the whole screen.
-->
<script lang="ts">
  import BellIcon from '@lucide/svelte/icons/bell';
  import CalendarCheckIcon from '@lucide/svelte/icons/calendar-check';
  import XIcon from '@lucide/svelte/icons/x';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { closeOnBack } from '$lib/layout/back.svelte.ts';
  import { pane } from '$lib/layout/room.svelte.ts';
  import { relativeTime } from '$lib/logic/format.ts';
  import { userDisplayName } from '$lib/logic/users.ts';
  import type { NotificationInbox } from '$lib/state/notification-inbox.svelte.ts';
  import type { Notification } from '$lib/state/notification-types.ts';
  import { Button } from '$lib/ui/button/index.js';
  import * as Dialog from '$lib/ui/dialog/index.js';
  import * as Popover from '$lib/ui/popover/index.js';

  import { bellLabel, notificationText, unreadBadge } from './notification-words.ts';

  interface Props {
    inbox: NotificationInbox;
    /** The language times are written in. */
    locale: string;
    /** The address of a notification's card or page, for its link. */
    hrefOf: (notification: Notification) => string;
    /** Goes to a notification's card or page; the bell has closed by then. */
    onGo: (notification: Notification) => void;
    /** Where a notification happened, "board · organization"; empty where that is not known. */
    sourceOf: (notification: Notification) => string;
  }

  let { inbox, locale, hrefOf, onGo, sourceOf }: Props = $props();

  /** The list as a sheet of the whole screen, as a narrow pane opens it. */
  let sheet = $state(false);

  // The back button closes the sheet, as the other dialogs: the page under it stays.
  closeOnBack(
    () => sheet,
    () => {
      sheetToggled(false);
    },
  );

  let open = $state(false);
  /** When the list was last shown: the times are written from it. */
  let now = $state(Date.now());
  /** Closed because a notification was gone to: the focus is the destination's, not the opener's. */
  let went = false;

  const label = $derived(bellLabel(inbox.unread));

  function nameOf(notification: Notification): string | undefined {
    const actorId = notification.subject?.actorId;
    const user = actorId ? inbox.people.get(actorId) : undefined;
    return user ? userDisplayName(user) : undefined;
  }

  /** The bell pressed opens the popover, or in a narrow pane the sheet in its place. */
  function toggled(next: boolean): void {
    if (next && pane.measured && !pane.columns) {
      sheetToggled(true);
      return;
    }
    open = next;
    shown(next);
  }

  function sheetToggled(next: boolean): void {
    sheet = next;
    shown(next);
  }

  /** The list is read while it is shown, the popover's or the sheet's. */
  function shown(next: boolean): void {
    if (next) {
      went = false;
      now = Date.now();
      void inbox.open();
    } else {
      inbox.close();
    }
  }

  function onclick(event: MouseEvent, notification: Notification): void {
    // A click with a key that opens a new tab is the browser's.
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.button !== 0) return;
    event.preventDefault();
    went = true;
    [open, sheet] = [false, false];
    inbox.close();
    onGo(notification);
  }

  function onCloseAutoFocus(event: Event): void {
    if (!went) return;
    went = false;
    event.preventDefault();
  }
</script>

<Popover.Root bind:open={() => open, toggled}>
  <Popover.Trigger>
    {#snippet child({ props }: { props: Record<string, unknown> })}
      <IconButton {...props} {label} data-testid="open-notifications" class="relative">
        <BellIcon />
        {#if inbox.unread > 0}
          <span
            class="absolute top-0.5 right-0.5 min-w-4 rounded-full bg-primary px-1 text-[0.625rem] leading-4 font-semibold text-primary-foreground tabular-nums"
            data-testid="unread-notifications"
            aria-hidden="true">{unreadBadge(inbox.unread)}</span
          >
        {/if}
      </IconButton>
    {/snippet}
  </Popover.Trigger>
  <Popover.Content
    align="end"
    role="dialog"
    aria-label={m.notifications_title()}
    data-testid="notifications"
    {onCloseAutoFocus}
    class="max-h-[min(32rem,calc(100dvh-4rem))] w-[min(24rem,calc(100vw-1rem-var(--safe-left)-var(--safe-right)))] gap-0 overflow-y-auto p-0"
  >
    <div class="flex items-center gap-2 border-b px-3 py-2">
      <h2 class="flex-1 font-semibold">{m.notifications_title()}</h2>
      {@render readAll()}
    </div>
    {@render list()}
  </Popover.Content>
</Popover.Root>

<Dialog.Root bind:open={() => sheet, sheetToggled}>
  <Dialog.Content
    data-testid="notifications"
    showCloseButton={false}
    {onCloseAutoFocus}
    class="top-0 left-0 flex h-dvh max-w-none translate-x-0 translate-y-0 flex-col gap-0 rounded-none p-0 pt-(--safe-top) pr-(--safe-right) pb-(--safe-bottom) pl-(--safe-left) ring-0 sm:max-w-none"
  >
    <Dialog.Header class="shrink-0 flex-row items-center gap-2 border-b py-2 pr-2 pl-4">
      <Dialog.Title class="min-w-0 flex-1 text-base font-semibold">
        {m.notifications_title()}
      </Dialog.Title>
      {@render readAll()}
      <Dialog.Close>
        {#snippet child({ props }: { props: Record<string, unknown> })}
          <IconButton {...props} label={m.ui_close()}>
            <XIcon />
          </IconButton>
        {/snippet}
      </Dialog.Close>
    </Dialog.Header>
    <div class="min-h-0 flex-1 overflow-y-auto">
      {@render list()}
    </div>
  </Dialog.Content>
</Dialog.Root>

{#snippet readAll()}
  <Button
    variant="ghost"
    size="sm"
    disabled={!inbox.listed || !inbox.items.some((one) => one.readAt === 0)}
    data-testid="read-all-notifications"
    onclick={() => void inbox.readAll()}>{m.notifications_read_all()}</Button
  >
{/snippet}

{#snippet list()}
  {#if !inbox.listed}
    <p class="p-4 text-muted-foreground">{m.notifications_loading()}</p>
  {:else if inbox.items.length === 0}
    <p class="p-4 text-muted-foreground" data-testid="no-notifications">
      {m.notifications_empty()}
    </p>
  {:else}
    <ul class="grid">
      {#each inbox.items as notification (notification.id)}
        {@const unread = notification.readAt === 0}
        <li
          data-notification={notification.id}
          data-unread={unread || undefined}
          data-kind={notification.subject?.kind}
        >
          {#if notification.subject}
            <a
              href={hrefOf(notification)}
              class="flex gap-2 px-3 py-2 hover:bg-accent focus-visible:bg-accent focus-visible:outline-none"
              onclick={(event: MouseEvent) => {
                onclick(event, notification);
              }}
            >
              {@render line(notification, unread)}
            </a>
          {:else}
            <div class="flex gap-2 px-3 py-2 text-muted-foreground">
              {@render line(notification, unread)}
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
{/snippet}

{#snippet line(notification: Notification, unread: boolean)}
  {@const source = sourceOf(notification)}
  <span
    class={['mt-1.5 size-2 shrink-0 rounded-full', unread ? 'bg-primary' : 'bg-transparent']}
    aria-hidden="true"
  ></span>
  <span class="grid min-w-0 flex-1 gap-0.5">
    <span class={['flex items-start gap-1.5', unread && 'font-medium']}>
      {#if notification.subject?.kind === 'review_due' || notification.subject?.kind === 'due_soon'}
        <CalendarCheckIcon class="mt-0.5 size-3.5 shrink-0 text-primary" aria-hidden="true" />
      {/if}
      <span>{notificationText(notification.subject, nameOf(notification))}</span>
    </span>
    {#if notification.subject?.agentTokenName}
      <span class="truncate text-xs text-muted-foreground" data-testid="notification-agent">
        {m.agent_badge({ name: notification.subject.agentTokenName })}
      </span>
    {/if}
    {#if source}
      <span class="truncate text-xs text-muted-foreground" data-testid="notification-source">
        {source}
      </span>
    {/if}
    <span class="text-xs text-muted-foreground">
      {relativeTime(notification.updatedAt, locale, Math.max(now, notification.updatedAt))}
      {#if unread}<span class="sr-only">· {m.notifications_unread()}</span>{/if}
    </span>
  </span>
{/snippet}
