<!--
  The strip: what is left of the sidebar where the whole list does not stand
  beside the board (a medium page, or an expanded one whose sidebar the
  reader folded). One button a board, in the order of the sidebar, so that
  another board is one tap away and the one on the page is marked. Above
  them the button that brings the whole list out; below them "Add board" and
  the reader's account.

  A board is drawn as its icon, or without one as the first character of its
  title. When there are more boards than fit, the boards scroll and the
  buttons above and below stay.
-->
<script lang="ts">
  import CalendarCheckIcon from '@lucide/svelte/icons/calendar-check';
  import ClockIcon from '@lucide/svelte/icons/clock';
  import PanelLeftOpenIcon from '@lucide/svelte/icons/panel-left-open';
  import PlusIcon from '@lucide/svelte/icons/plus';
  import SunIcon from '@lucide/svelte/icons/sun';

  import Avatar from '$lib/components/avatar/Avatar.svelte';
  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { ContainerPlace } from '$lib/logic/urls.ts';
  import { isGuest, type Workspace } from '$lib/state/index.ts';

  import { namedAs, shownTitle } from './shown-title.ts';
  import ContainerIcon from './ContainerIcon.svelte';

  interface Props {
    workspace: Workspace;
    /** The board or wiki on the page. */
    containerId: string | undefined;
    meTime?: boolean;
    timeHref?: string | undefined;
    onOpenTime?: (() => void) | undefined;
    todayHub?: { active: boolean; onOpen: () => void } | undefined;
    reviewsToday?: { active: boolean; onOpen: () => void } | undefined;
    learningToday?: { active: boolean; onOpen: () => void } | undefined;
    containerHref: (container: ContainerPlace) => string;
    /** Out of the layout: the list stands beside the board, or the page is compact. */
    hidden: boolean;
    /** Behind the open drawer: neither pressed nor reached with the keyboard. */
    inert: boolean;
    onOpenContainer: (container: ContainerPlace) => void;
    onExpand: (event: MouseEvent) => void;
    onAddBoard: () => void;
    onOpenSettings: () => void;
  }

  let {
    workspace,
    containerId,
    meTime = false,
    timeHref,
    onOpenTime,
    todayHub,
    reviewsToday,
    learningToday,
    containerHref,
    hidden,
    inert,
    onOpenContainer,
    onExpand,
    onAddBoard,
    onOpenSettings,
  }: Props = $props();

  const containers = $derived(workspace.sidebar.flatMap((entry) => entry.containers));
  const titleOf = shownTitle;
  const myName = $derived(
    (workspace.me && workspace.userName(workspace.me.id)) ?? m['centerPanel.unknown-user'](),
  );

  function open(event: MouseEvent, container: ContainerPlace): void {
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) {
      return;
    }
    event.preventDefault();
    onOpenContainer(container);
  }
</script>

<nav
  data-testid="sidebar-strip"
  aria-label={m.shell_boards()}
  {inert}
  class={[
    'w-14 shrink-0 flex-col items-center gap-1 border-r bg-sidebar pt-2 pb-[calc(0.5rem+var(--safe-bottom))] text-sidebar-foreground',
    hidden ? 'hidden' : 'flex',
  ]}
>
  <IconButton label={m.shell_expand_sidebar()} data-testid="expand-sidebar" onclick={onExpand}>
    <PanelLeftOpenIcon />
  </IconButton>
  {#if todayHub && workspace.me?.id}
    <IconButton
      label={m.today_hub_title()}
      aria-current={todayHub.active ? 'page' : undefined}
      data-testid="strip-open-today-hub"
      class={todayHub.active ? 'bg-sidebar-accent ring-1 ring-input-border' : undefined}
      onclick={todayHub.onOpen}
    >
      <SunIcon />
    </IconButton>
  {/if}
  <div class="my-1 w-8 shrink-0 border-t"></div>
  <ul
    class="flex min-h-0 w-full flex-1 [scrollbar-width:none] flex-col items-center gap-1 overflow-y-auto overscroll-contain py-1"
  >
    {#each containers as container (container.id)}
      {@const title = titleOf(container)}
      <li class="shrink-0">
        <a
          href={containerHref(container)}
          aria-label={namedAs(container.kind, title) ?? title}
          {title}
          aria-current={container.id === containerId ? 'page' : undefined}
          class="grid size-8 place-items-center rounded-lg hover:bg-sidebar-accent aria-[current=page]:bg-sidebar-accent aria-[current=page]:ring-1 aria-[current=page]:ring-input-border roomy:size-touch"
          onclick={(event) => {
            open(event, container);
          }}
        >
          <ContainerIcon
            icon={container.icon}
            kind={container.kind}
            {title}
            locale={workspace.locale}
          />
        </a>
      </li>
    {/each}
  </ul>
  <div class="my-1 w-8 shrink-0 border-t"></div>
  {#if reviewsToday && workspace.backend.course && workspace.info && workspace.me?.id}
    <IconButton
      label={m.today_reviews_title()}
      aria-current={reviewsToday.active ? 'page' : undefined}
      data-testid="strip-open-today-reviews"
      class={reviewsToday.active ? 'bg-sidebar-accent ring-1 ring-input-border' : undefined}
      onclick={reviewsToday.onOpen}
    >
      <CalendarCheckIcon />
    </IconButton>
  {/if}
  {#if learningToday && workspace.backend.course && workspace.info && workspace.me?.id}
    <IconButton
      label={m.learning_today_title()}
      aria-current={learningToday.active ? 'page' : undefined}
      data-testid="strip-open-learning-today"
      class={learningToday.active ? 'bg-sidebar-accent ring-1 ring-input-border' : undefined}
      onclick={learningToday.onOpen}
    >
      <CalendarCheckIcon />
    </IconButton>
  {/if}
  {#if timeHref && workspace.me?.id}
    <IconButton
      label={m.time_title()}
      aria-current={meTime ? 'page' : undefined}
      data-testid="strip-open-my-time"
      class={meTime ? 'bg-sidebar-accent ring-1 ring-input-border' : undefined}
      onclick={() => {
        onOpenTime?.();
      }}
    >
      <ClockIcon />
    </IconButton>
  {/if}
  <!-- A guest is on the boards they were invited to, and makes none; nobody where none is shown. -->
  {#if workspace.info && !isGuest(workspace)}
    <IconButton label={m.shell_add_board()} data-testid="strip-add-board" onclick={onAddBoard}>
      <PlusIcon />
    </IconButton>
  {/if}
  <IconButton
    label={m.shell_account_of({ name: myName })}
    aria-haspopup="dialog"
    data-testid="strip-open-settings"
    onclick={onOpenSettings}
  >
    <Avatar
      name={myName}
      colorKey={workspace.me?.id}
      locale={workspace.locale}
      decorative
      class="size-6"
    />
  </IconButton>
</nav>
