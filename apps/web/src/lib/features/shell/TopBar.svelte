<!--
  The top row of the pane: which space this is in (`SpaceBadge`), which board (or wiki) this is, who is on it, sharing it
  (for its admins), and undo and redo, which pulse (or fade, with less
  motion) while one asked for waits for a write.

  What it shows follows the width of the pane it is in, not of the window:
  five faces of who is here from where two kanban columns fit (two below).
  The description is behind "ⓘ" next to the title (a line of "…" in a
  narrow pane), so that the title keeps the room (ContainerDescription.svelte);
  a board's editors and a wiki's admins change it there, or add one. The number of the board's members, there at every width, opens its members
  in the sharing dialog, which shows them alone to a reader who is no admin.

  Its editors find the board's deleted cards here ("Deleted cards").

  A narrow pane (a phone) has room for the title only if undo, redo, the
  description and the deleted cards are lines of a menu ("…"); a phone seldom has a keyboard for
  their keys, which work all the same; so is sharing, which a board's admins seldom need there. From
  where two kanban columns fit they are buttons of their own. The bell (`search`) stays a button of
  the bar at every width.

  The title is a field for a reader who may change the board, and plain text
  for one who may not; a board that can only be read says so next to it. The
  button that slides the sidebar in stands at the left where nothing else on
  the page opens it (a compact page).
-->
<script lang="ts">
  import CalendarCheckIcon from '@lucide/svelte/icons/calendar-check';
  import ClockIcon from '@lucide/svelte/icons/clock';
  import EllipsisIcon from '@lucide/svelte/icons/ellipsis';
  import InfoIcon from '@lucide/svelte/icons/info';
  import MenuIcon from '@lucide/svelte/icons/menu';
  import Redo2Icon from '@lucide/svelte/icons/redo-2';
  import Share2Icon from '@lucide/svelte/icons/share-2';
  import SunIcon from '@lucide/svelte/icons/sun';
  import Trash2Icon from '@lucide/svelte/icons/trash-2';
  import Undo2Icon from '@lucide/svelte/icons/undo-2';
  import type { Snippet } from 'svelte';

  import { shortcutKeys, shortcutLabel } from '$lib/keys.ts';
  import PresenceRoster from './PresenceRoster.svelte';
  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import ActionMenu from '$lib/components/menu/ActionMenu.svelte';
  import type { MenuAction } from '$lib/components/menu/actions.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { cardTrash, isOwnSpace, type Trash, type Workspace } from '$lib/state/index.ts';

  import { Button } from '$lib/ui/button/index.js';

  import ContainerDescription from './ContainerDescription.svelte';
  import ContainerIcon from './ContainerIcon.svelte';
  import BoardTitle from './BoardTitle.svelte';
  import ShareDialog from './ShareDialog.svelte';
  import SpaceBadge from './SpaceBadge.svelte';
  import { namedAs, shownTitle } from './shown-title.ts';
  import TrashDialog from './TrashDialog.svelte';

  interface Props {
    workspace: Workspace;
    /** The board or wiki on the page. */
    containerId: string | undefined;
    /** Whether the personal "My time" screen is shown. */
    meTime?: boolean;
    /** Whether the unified account "Today" hub is shown. */
    todayHub?: boolean;
    /** Whether the workspace "Today's reviews" screen is shown. */
    reviewsToday?: boolean;
    /** Whether the workspace "Today's learning" screen is shown. */
    learningToday?: boolean;
    /** The address of a board's or a wiki's public link with its read token. */
    sharedHref: (containerId: string, readToken: string) => string;
    /** Show the button that opens the drawer. */
    menuButton: boolean;
    onOpenSidebar: (event: MouseEvent) => void;
    /** The buttons of the search and of the notifications, where there are any. */
    search?: Snippet<[Workspace]>;
    /** Kept away: a drawer of the screen below is out. */
    inert?: boolean;
  }

  let {
    workspace,
    containerId,
    meTime = false,
    todayHub = false,
    reviewsToday = false,
    learningToday = false,
    sharedHref,
    menuButton,
    onOpenSidebar,
    search,
    inert = false,
  }: Props = $props();

  let sharing = $state(false);
  /** The sharing dialog was opened from the number of members: it begins at them. */
  let sharingMembers = $state(false);
  let trashOpen = $state(false);
  /** The board or wiki whose description is open (ContainerDescription.svelte): another starts closed. */
  let describingIn = $state<string | undefined>(undefined);
  /** The deleted cards of the board, read anew each time they are shown. */
  let trash = $state.raw<Trash | undefined>(undefined);

  /** The board as the list of boards has it: known before the board itself is read. */
  const listed = $derived(workspace.containers.find((candidate) => candidate.id === containerId));
  /** The open board, once it is read: who is on it and what the reader may do. */
  const store = $derived(
    workspace.board?.boardId === containerId && workspace.board?.status === 'ready'
      ? workspace.board
      : undefined,
  );
  /** The open wiki, where the page is one: its admins change its description. */
  const wiki = $derived.by(() => {
    const active = workspace.activeContainer;
    return active?.kind === 'wiki' && active.wikiId === containerId ? active : undefined;
  });
  /** The open course, where the page is one: its admins change its description. */
  const course = $derived.by(() => {
    const active = workspace.activeContainer;
    return active?.kind === 'course' && active.courseId === containerId ? active : undefined;
  });
  /** The container as it is read where it is open, else as the list has it. */
  const container = $derived(store?.board ?? wiki?.wiki ?? course?.course ?? listed);
  /** Sharing is for the board's admins; a template is shared as it is made, never joined. */
  const canShare = $derived(store?.canShare === true && store.board?.isTemplate !== true);
  /** The trash of cards is for the board's editors, as the server has it. */
  const canRestoreCards = $derived(store?.canEdit === true);

  /** Changes the description, for whoever may: a board's editors, a wiki's or course's admins. */
  const describe = $derived.by(() => {
    if (store?.canEdit) {
      const board = store;
      return (description: string) =>
        void board.changeBoard(
          (current) => ({ ...current, description }),
          'changeBoardDescription',
        );
    }
    if (wiki?.canManage) {
      const open = wiki;
      return (description: string) => void open.changeWiki({ description });
    }
    if (course?.canManage) {
      const open = course;
      return (description: string) => void open.changeCourse({ description });
    }
    return undefined;
  });

  /** The narrow pane's menu: the same undo, redo and trash as the buttons it stands for. */
  const narrowActions = $derived.by<MenuAction[]>(() => {
    const list: MenuAction[] = [
      {
        id: 'undo',
        label: m['UndoRedoHotKeys.canUndo'](),
        icon: Undo2Icon,
        disabled: !workspace.canUndo,
        run: () => void workspace.undoLast(),
      },
      {
        id: 'redo',
        label: m['UndoRedoHotKeys.canRedo'](),
        icon: Redo2Icon,
        disabled: !workspace.canRedo,
        run: () => void workspace.redoLast(),
      },
    ];
    if (container?.description || describe) {
      list.push({
        id: 'description',
        label: container?.description ? m.shell_description() : m.shell_description_add(),
        icon: InfoIcon,
        separated: true,
        run: () => {
          describingIn = containerId;
        },
      });
    }
    if (canShare) {
      list.push({
        id: 'share',
        label: m.shell_share(),
        icon: Share2Icon,
        separated: true,
        afterClose: () => {
          sharingMembers = false;
          sharing = true;
        },
      });
    }
    if (canRestoreCards) {
      list.push({
        id: 'card-trash',
        label: m.shell_trash_cards(),
        icon: Trash2Icon,
        separated: true,
        run: openTrash,
      });
    }
    return list;
  });

  function openTrash(): void {
    if (!store) return;
    trash = cardTrash(store);
    trashOpen = true;
    void trash.read();
  }
</script>

<header
  data-testid="top-bar"
  {inert}
  class="flex h-12 shrink-0 items-center gap-1 border-b px-1.5 @columns/pane:h-14 @columns/pane:gap-2 @columns/pane:px-3"
>
  {#if menuButton}
    <IconButton label={m.shell_open_sidebar()} data-testid="open-sidebar" onclick={onOpenSidebar}>
      <MenuIcon />
    </IconButton>
  {/if}

  <SpaceBadge {workspace} />

  {#if meTime}
    <div class="flex min-w-0 shrink items-center gap-2 pl-1">
      <ClockIcon class="size-5 shrink-0 text-muted-foreground" aria-hidden="true" />
      <h1
        class="min-w-0 truncate text-base font-semibold @columns/pane:text-lg"
        data-testid="top-bar-time-title"
      >
        {m.time_title()}
      </h1>
    </div>
  {:else if todayHub}
    <div class="flex min-w-0 shrink items-center gap-2 pl-1">
      <SunIcon class="size-5 shrink-0 text-muted-foreground" aria-hidden="true" />
      <h1
        class="min-w-0 truncate text-base font-semibold @columns/pane:text-lg"
        data-testid="top-bar-today-hub-title"
      >
        {m.today_hub_title()}
      </h1>
    </div>
  {:else if reviewsToday}
    <div class="flex min-w-0 shrink items-center gap-2 pl-1">
      <CalendarCheckIcon class="size-5 shrink-0 text-muted-foreground" aria-hidden="true" />
      <h1
        class="min-w-0 truncate text-base font-semibold @columns/pane:text-lg"
        data-testid="top-bar-reviews-title"
      >
        {m.today_reviews_title()}
      </h1>
    </div>
  {:else if learningToday}
    <div class="flex min-w-0 shrink items-center gap-2 pl-1">
      <CalendarCheckIcon class="size-5 shrink-0 text-muted-foreground" aria-hidden="true" />
      <h1
        class="min-w-0 truncate text-base font-semibold @columns/pane:text-lg"
        data-testid="top-bar-learning-today-title"
      >
        {m.learning_today_title()}
      </h1>
    </div>
  {:else if container}
    <div class="flex min-w-0 shrink items-center gap-1.5 pl-1">
      <ContainerIcon
        icon={container.icon}
        kind={container.kind}
        title={container.title}
        locale={workspace.locale}
      />
      <h1
        class="flex min-w-0 shrink text-base font-semibold @columns/pane:text-lg"
        aria-label={namedAs(container.kind, shownTitle(container))}
      >
        {#if store?.canEdit}
          {#key container.id}
            <BoardTitle
              title={container.title}
              onChange={(title: string) =>
                void store.changeBoard((current) => ({ ...current, title }), 'changeBoardTitle')}
            />
          {/key}
        {:else}
          <span data-testid="board-title-text" class="min-w-0 truncate px-1.5">
            <!-- The name of a board without one is the same wherever a name is shown; the field's placeholder is the other message. -->
            {shownTitle(container)}
          </span>
        {/if}
      </h1>
    </div>
    {#if container.description || describe}
      {#key container.id}
        <ContainerDescription
          bind:open={
            () => describingIn === container.id,
            (open: boolean) => {
              describingIn = open ? container.id : undefined;
            }
          }
          description={container.description}
          onChange={describe}
        />
      {/key}
    {/if}
    {#if container.isTemplate}
      <span
        data-testid="template-note"
        title={m['Workspace.editing-board-template']()}
        class="shrink-0 rounded-md border border-dashed px-1.5 py-0.5 text-xs text-muted-foreground"
      >
        <span aria-hidden="true">{m.shell_template_note()}</span>
        <span class="sr-only">{m['Workspace.editing-board-template']()}</span>
      </span>
    {/if}
    {#if store && !store.canEdit}
      <span
        data-testid="read-only"
        class="shrink-0 rounded-md border px-1.5 py-0.5 text-xs text-muted-foreground"
      >
        {m.shared_read_only()}
      </span>
    {/if}
  {/if}

  <div class="min-w-0 flex-1"></div>

  <!-- The own space has its owner alone: nobody to see here, no members to count. -->
  {#if store && !store.missing && !isOwnSpace(workspace.info)}
    <PresenceRoster
      board={store}
      onMembers={() => {
        sharingMembers = true;
        sharing = true;
      }}
    />
  {/if}

  {#if canShare}
    <Button
      variant="outline"
      size="sm"
      data-testid="share-board"
      aria-haspopup="dialog"
      class="shrink-0 @max-columns/pane:hidden roomy:min-h-touch roomy:min-w-touch"
      onclick={() => {
        sharingMembers = false;
        sharing = true;
      }}
    >
      <Share2Icon aria-hidden="true" />
      <span class="sr-only @header/pane:not-sr-only">{m.shell_share()}</span>
    </Button>
  {/if}

  {#if canRestoreCards}
    <IconButton
      label={m.shell_trash_cards()}
      data-testid="open-card-trash"
      aria-haspopup="dialog"
      class="@max-columns/pane:hidden"
      onclick={openTrash}
    >
      <Trash2Icon />
    </IconButton>
  {/if}

  {@render search?.(workspace)}

  <!-- An undo or redo waiting for a write says so on both (on the menu in a narrow pane), until it is carried out. -->
  <IconButton
    label={m['UndoRedoHotKeys.canUndo']()}
    data-testid="undo"
    title={m['UndoRedoHotKeys.canUndo']() + ' (' + shortcutLabel('Z') + ')'}
    aria-keyshortcuts={shortcutKeys('Z')}
    disabled={!workspace.canUndo}
    aria-busy={workspace.undoBusy}
    class="aria-busy:motion-safe:animate-pulse aria-busy:motion-reduce:opacity-60 @max-columns/pane:hidden"
    onclick={() => void workspace.undoLast()}
  >
    <Undo2Icon />
  </IconButton>
  <IconButton
    label={m['UndoRedoHotKeys.canRedo']()}
    data-testid="redo"
    title={m['UndoRedoHotKeys.canRedo']() + ' (' + shortcutLabel('Z', { shift: true }) + ')'}
    aria-keyshortcuts={shortcutKeys('Z', { shift: true })}
    disabled={!workspace.canRedo}
    aria-busy={workspace.undoBusy}
    class="aria-busy:motion-safe:animate-pulse aria-busy:motion-reduce:opacity-60 @max-columns/pane:hidden"
    onclick={() => void workspace.redoLast()}
  >
    <Redo2Icon />
  </IconButton>
  <ActionMenu title={m.shell_bar_more()} actions={narrowActions}>
    {#snippet trigger(props: Record<string, unknown>)}
      <IconButton
        {...props}
        label={m.shell_bar_more()}
        data-testid="bar-menu"
        aria-busy={workspace.undoBusy}
        class="aria-busy:motion-safe:animate-pulse aria-busy:motion-reduce:opacity-60 @columns/pane:hidden"
      >
        <EllipsisIcon />
      </IconButton>
    {/snippet}
  </ActionMenu>
</header>

{#if store && canRestoreCards}
  <TrashDialog
    bind:open={trashOpen}
    {trash}
    {workspace}
    title={m.shell_trash_cards()}
    description={m.shell_trash_cards_retention({ days: workspace.trashRetentionDays })}
    untitled={m['KanbanCard.untitled']()}
    testId="card-trash"
  />
{/if}

<!-- Whoever reads the board may see its members; only its admins change them or share it. -->
{#if store?.board}
  <ShareDialog
    bind:open={sharing}
    {workspace}
    shareable={store}
    membersOnly={!canShare}
    readOnly={!canShare}
    at={sharingMembers ? 'members' : undefined}
    linkOf={(readToken: string) =>
      new URL(sharedHref(store.boardId, readToken), location.href).href}
  />
{/if}
