<!--
  One line of the notifications list: a link to its card or page where the
  reader reads that (a plain click goes there, `onGo`), else a line that says
  nothing of it; and, at its end, a menu of 44×44 (`ActionMenu`) that marks
  it read or unread and puts it away or back. A finger swipes it to the left
  to put it away (`swipeAway`), and back from the archive.

  Each line, link or not, is reached by J and K from the list's keys
  (`[data-row-focus]`, `inbox-keys.ts`), and takes no Tab stop of its own
  where it is no link: its menu does.
-->
<script lang="ts">
  import ArchiveIcon from '@lucide/svelte/icons/archive';
  import ArchiveRestoreIcon from '@lucide/svelte/icons/archive-restore';
  import EllipsisIcon from '@lucide/svelte/icons/ellipsis';
  import MailIcon from '@lucide/svelte/icons/mail';
  import MailOpenIcon from '@lucide/svelte/icons/mail-open';
  import type { Snippet } from 'svelte';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import ActionMenu from '$lib/components/menu/ActionMenu.svelte';
  import type { MenuAction } from '$lib/components/menu/actions.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { Notification } from '$lib/state/notification-types.ts';

  import type { RowActions } from './row-actions.ts';
  import { swipeAway } from './swipe-away.ts';

  interface Props {
    notification: Notification;
    /** What the line says, for the link and the menu's name. */
    text: string;
    href: string | undefined;
    actions: RowActions;
    /** A plain click on the link. */
    onclick: (event: MouseEvent) => void;
    line: Snippet;
  }

  let { notification, text, href, actions, onclick, line }: Props = $props();

  const unread = $derived(notification.readAt === 0);
  const archived = $derived(notification.archivedAt !== 0);

  const menu = $derived<MenuAction[]>([
    unread
      ? {
          id: 'read',
          label: m.notifications_mark_read(),
          icon: MailOpenIcon,
          run: () => {
            actions.toggleRead(notification);
          },
        }
      : {
          id: 'unread',
          label: m.notifications_mark_unread(),
          icon: MailIcon,
          run: () => {
            actions.toggleRead(notification);
          },
        },
    archived
      ? {
          id: 'unarchive',
          label: m.notifications_unarchive(),
          icon: ArchiveRestoreIcon,
          run: () => {
            actions.toggleArchive(notification);
          },
        }
      : {
          id: 'archive',
          label: m.notifications_archive(),
          icon: ArchiveIcon,
          run: () => {
            actions.toggleArchive(notification);
          },
        },
  ]);
</script>

<li
  data-notification={notification.id}
  data-unread={unread || undefined}
  data-kind={notification.subject?.kind}
  class="relative flex items-stretch bg-popover"
  {@attach swipeAway(() => {
    actions.toggleArchive(notification);
  })}
>
  {#if href !== undefined}
    <a
      {href}
      data-row-focus
      class="flex min-w-0 flex-1 gap-2 py-2 pl-3 hover:bg-accent focus-visible:bg-accent focus-visible:outline-none"
      {onclick}
    >
      {@render line()}
    </a>
  {:else}
    <div
      data-row-focus
      tabindex="-1"
      class="flex min-w-0 flex-1 gap-2 py-2 pl-3 text-muted-foreground focus-visible:bg-accent focus-visible:outline-none"
    >
      {@render line()}
    </div>
  {/if}
  <ActionMenu title={m.notifications_row_menu()} subtitle={text} align="end" actions={menu}>
    {#snippet trigger(props: Record<string, unknown>)}
      <IconButton
        {...props}
        label={m.notifications_row_menu()}
        data-testid="notification-menu"
        class="size-touch shrink-0 self-center text-muted-foreground"
      >
        <EllipsisIcon aria-hidden="true" />
      </IconButton>
    {/snippet}
  </ActionMenu>
</li>
