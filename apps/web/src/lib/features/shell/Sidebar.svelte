<!--
  The sidebar: workspace switcher, search, categories with their containers,
  and footer navigation/actions (`SidebarDialogs.svelte` holds the dialogs).
-->
<script lang="ts">
  import { onDestroy } from 'svelte';

  import BuildingIcon from '@lucide/svelte/icons/building-2';
  import CalendarCheckIcon from '@lucide/svelte/icons/calendar-check';
  import ClockIcon from '@lucide/svelte/icons/clock';
  import NotebookPenIcon from '@lucide/svelte/icons/notebook-pen';
  import PanelLeftCloseIcon from '@lucide/svelte/icons/panel-left-close';
  import PlusIcon from '@lucide/svelte/icons/plus';
  import SearchIcon from '@lucide/svelte/icons/search';
  import SunIcon from '@lucide/svelte/icons/sun';
  import Trash2Icon from '@lucide/svelte/icons/trash-2';
  import UsersIcon from '@lucide/svelte/icons/users';
  import XIcon from '@lucide/svelte/icons/x';

  import Avatar from '$lib/components/avatar/Avatar.svelte';
  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { ContainerPlace } from '$lib/logic/urls.ts';
  import type { Container } from '$lib/logic/wire.ts';
  import {
    administers,
    containerTrash,
    firstDiary,
    isGuest,
    isOwnSpace,
    organizationNames,
    peopleOf,
    whileHere,
    type Category,
    type Trash,
    type Workspace,
  } from '$lib/state/index.ts';
  import { Input } from '$lib/ui/input/index.js';

  import { BoardSearch } from './board-search.svelte.ts';
  import type { SidebarAsked } from './sidebar-actions.ts';
  import SidebarDialogs from './SidebarDialogs.svelte';
  import type { FavoritePlaces } from './favorite-places.ts';
  import SidebarList from './SidebarList.svelte';
  import SidebarNavButton from './SidebarNavButton.svelte';
  import WorkspaceSwitcher from './WorkspaceSwitcher.svelte';

  interface Props {
    workspace: Workspace;
    containerId: string | undefined;
    meTime?: boolean;
    timeHref?: string | undefined;
    onOpenTime?: (() => void) | undefined;
    todayHub?: { active: boolean; onOpen: () => void } | undefined;
    reviewsToday?: { active: boolean; onOpen: () => void } | undefined;
    learningToday?: { active: boolean; onOpen: () => void } | undefined;
    containerHref: (container: ContainerPlace) => string;
    /** Where the reader's stars lead; none, no stars. */
    favoritePlaces?: FavoritePlaces | undefined;
    beside: boolean;
    onOpenContainer: (container: ContainerPlace) => void;
    onClose: () => void;
    onAddBoard: () => void;
    onOpenSettings: () => void;
    orgHref?: string | undefined;
    onSwitchWorkspace?: ((workspaceId: string) => void) | undefined;
    onCreateOrganization?: ((name: string) => Promise<void>) | undefined;
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
    favoritePlaces,
    beside,
    onOpenContainer,
    onClose,
    onAddBoard,
    onOpenSettings,
    orgHref,
    onSwitchWorkspace,
    onCreateOrganization,
  }: Props = $props();

  const shown = $derived(workspace.info ?? workspace.closed);
  const ownSpace = $derived(isOwnSpace(shown));
  const orgNames = $derived(organizationNames(workspace));
  $effect(() => {
    orgNames.follow(workspace.workspaces);
  });

  let query = $state('');
  const boardSearch = new BoardSearch(async (text) =>
    isGuest(workspace)
      ? []
      : (await workspace.searchBoards(text)).filter((board) =>
          workspace.workspaces.some((one) => one.id === board.workspaceId),
        ),
  );
  onDestroy(() => {
    boardSearch.stop();
  });

  function setQuery(text: string): void {
    query = text;
    boardSearch.run(text);
  }

  const myName = $derived(
    (workspace.me && workspace.userName(workspace.me.id)) ?? m['centerPanel.unknown-user'](),
  );

  const canDeleteOpenContainer = $derived.by(() => {
    const active = workspace.activeContainer;
    return (
      (workspace.board?.boardId === containerId && workspace.board?.canDelete === true) ||
      (active?.kind === 'wiki' && active.wikiId === containerId && active.canDelete) ||
      (active?.kind === 'course' && active.courseId === containerId && active.canDelete) ||
      workspace.containers.some((one) => one.id === containerId && one.kind === 'diary')
    );
  });

  const openContainerJoinable = $derived.by(() => {
    const active = workspace.activeContainer;
    return (
      (workspace.board?.boardId === containerId && workspace.board?.joinable === true) ||
      (active?.kind === 'wiki' && active.wikiId === containerId && active.joinable) ||
      (active?.kind === 'course' && active.courseId === containerId && active.joinable)
    );
  });

  const firstDiaryOffered = $derived(
    ownSpace &&
      workspace.backend.diary !== undefined &&
      !workspace.containers.some((one) => one.kind === 'diary'),
  );

  async function openFirstDiary(): Promise<void> {
    const made = await whileHere(workspace, () => firstDiary(workspace));
    if (made) onOpenContainer(made);
  }

  let naming = $state.raw<{ mode: 'create' } | { mode: 'rename'; category: Category }>({
    mode: 'create',
  });
  let namingOpen = $state(false);

  function submitName(name: string): void {
    if (naming.mode === 'create') void workspace.createCategory(name);
    else if (name !== naming.category.name) void workspace.renameCategory(naming.category.id, name);
  }

  const access = $derived(peopleOf(workspace));
  let peopleOpen = $state(false);
  let trashOpen = $state(false);
  let trash = $state.raw<Trash | undefined>(undefined);

  function openTrash(): void {
    trash = containerTrash(workspace);
    trashOpen = true;
    void trash.read();
  }

  let asked = $state.raw<SidebarAsked | undefined>(undefined);
  let askOpen = $state(false);
  let copyingContainer = $state.raw<Container | undefined>(undefined);
  let copyOpen = $state(false);

  async function deleteContainer(container: Container): Promise<void> {
    const here = await whileHere(workspace, async () => {
      await workspace.deleteContainer(container.id);
      return true;
    });
    if (
      !here ||
      container.id !== containerId ||
      workspace.containers.some((left) => left.id === container.id)
    ) {
      return;
    }
    const next = workspace.sidebar.flatMap((entry) => entry.containers)[0];
    if (next) onOpenContainer(next);
  }

  async function templateFromBoard(boardId: string): Promise<void> {
    const made = await whileHere(workspace, () => workspace.createTemplateFromBoard(boardId));
    if (made) onOpenContainer(made);
  }

  async function duplicateContainer(containerId: string): Promise<void> {
    const made = await whileHere(workspace, () => workspace.duplicateContainer(containerId));
    if (made) onOpenContainer(made);
  }

  function confirmed(): void {
    if (asked?.kind === 'container') void deleteContainer(asked.container);
    else if (asked?.kind === 'category') void workspace.deleteCategory(asked.category.id);
  }
</script>

<div class="flex shrink-0 items-center gap-1 px-2 pt-2 pb-1">
  {#if onSwitchWorkspace && (workspace.workspaces.length > 1 || onCreateOrganization)}
    <WorkspaceSwitcher
      workspaces={workspace.workspaces}
      current={shown?.id}
      orgName={(orgId: string | undefined) => orgNames.of(orgId)}
      onSwitch={onSwitchWorkspace}
      {onCreateOrganization}
    />
  {:else}
    <span class="min-w-0 flex-1 truncate pl-2 text-lg font-semibold tracking-tight" translate="no">
      {m.app_name()}
    </span>
  {/if}
  <IconButton
    label={beside ? m.shell_collapse_sidebar() : m.shell_close_sidebar()}
    data-testid="close-sidebar"
    onclick={onClose}
  >
    {#if beside}<PanelLeftCloseIcon />{:else}<XIcon />{/if}
  </IconButton>
</div>

<div class="relative shrink-0 px-2 py-1">
  <SearchIcon
    class="pointer-events-none absolute top-1/2 left-4.5 size-4 -translate-y-1/2 text-muted-foreground"
    aria-hidden="true"
  />
  <Input
    bind:value={query}
    type="text"
    inputmode="search"
    enterkeyhint="search"
    autocomplete="off"
    autocapitalize="off"
    spellcheck={false}
    aria-label={m.shell_find_board()}
    placeholder={m.shell_find_board()}
    data-testid="sidebar-search"
    oninput={(event: Event) => {
      boardSearch.run((event.currentTarget as HTMLInputElement).value);
    }}
    class="bg-background pr-9 pl-8 roomy:h-touch roomy:pr-12 roomy:text-base"
    onkeydown={(event: KeyboardEvent) => {
      if (event.key !== 'Escape' || query === '') return;
      event.preventDefault();
      setQuery('');
    }}
  />
  {#if query !== ''}
    <IconButton
      label={m.shell_find_board_clear()}
      class="absolute top-1 right-2 text-muted-foreground"
      onclick={() => {
        setQuery('');
      }}
    >
      <XIcon />
    </IconButton>
  {/if}
</div>

{#if todayHub && workspace.me?.id}
  <div class="shrink-0 px-2 pb-1">
    <SidebarNavButton
      testId="open-today-hub"
      active={todayHub.active}
      label={m.today_hub_title()}
      onclick={todayHub.onOpen}
      icon={SunIcon}
    />
  </div>
{/if}

<nav class="flex min-h-0 flex-1 flex-col" aria-label={m.shell_boards()}>
  <SidebarList
    {workspace}
    {containerId}
    {containerHref}
    {favoritePlaces}
    {query}
    found={boardSearch.found}
    searching={boardSearch.pending}
    {canDeleteOpenContainer}
    {openContainerJoinable}
    {onOpenContainer}
    onNewCategory={() => {
      naming = { mode: 'create' };
      namingOpen = true;
    }}
    onRenameCategory={(category: Category) => {
      naming = { mode: 'rename', category };
      namingOpen = true;
    }}
    onDeleteCategory={(category: Category) => {
      asked = { kind: 'category', category };
      askOpen = true;
    }}
    onTemplateFromBoard={(id: string) => void templateFromBoard(id)}
    onDuplicateContainer={(id: string) => void duplicateContainer(id)}
    onCopyToWorkspace={(container: Container) => {
      copyingContainer = container;
      copyOpen = true;
    }}
    onDeleteContainer={(container: Container) => {
      asked = { kind: 'container', container };
      askOpen = true;
    }}
  />
</nav>

<div class="shrink-0 border-t p-2 pb-[calc(0.5rem+var(--safe-bottom))]">
  {#if workspace.info && !isGuest(workspace)}
    {#if firstDiaryOffered}
      <button
        type="button"
        data-testid="open-diary"
        class="flex min-h-8 w-full items-center gap-2 rounded-md px-2 text-left hover:bg-sidebar-accent roomy:min-h-touch"
        onclick={() => void openFirstDiary()}
      >
        <NotebookPenIcon class="size-5 shrink-0 p-0.5" aria-hidden="true" />
        <span class="min-w-0 flex-1 truncate">{m.shell_open_diary()}</span>
      </button>
    {/if}
    <button
      type="button"
      data-testid="add-board"
      class="flex min-h-8 w-full items-center gap-2 rounded-md px-2 text-left hover:bg-sidebar-accent roomy:min-h-touch"
      onclick={onAddBoard}
    >
      <PlusIcon class="size-5 shrink-0 p-0.5" aria-hidden="true" />
      <span class="min-w-0 flex-1 truncate">{m.shell_add_board()}</span>
    </button>
    <button
      type="button"
      data-testid="open-trash"
      aria-haspopup="dialog"
      class="flex min-h-8 w-full items-center gap-2 rounded-md px-2 text-left hover:bg-sidebar-accent roomy:min-h-touch"
      onclick={openTrash}
    >
      <Trash2Icon class="size-5 shrink-0 p-0.5" aria-hidden="true" />
      <span class="min-w-0 flex-1 truncate">{m.shell_trash()}</span>
    </button>
  {/if}
  {#if access && !isGuest(workspace) && !ownSpace && (administers(workspace) || access.people.teams)}
    <button
      type="button"
      data-testid="open-people"
      aria-haspopup="dialog"
      class="flex min-h-8 w-full items-center gap-2 rounded-md px-2 text-left hover:bg-sidebar-accent roomy:min-h-touch"
      onclick={() => {
        peopleOpen = true;
      }}
    >
      <UsersIcon class="size-5 shrink-0 p-0.5" aria-hidden="true" />
      <span class="min-w-0 flex-1 truncate">{m.people_title()}</span>
    </button>
  {/if}
  {#if reviewsToday && workspace.backend.course && workspace.info && workspace.me?.id}
    <SidebarNavButton
      testId="open-today-reviews"
      active={reviewsToday.active}
      label={m.today_reviews_title()}
      onclick={reviewsToday.onOpen}
      icon={CalendarCheckIcon}
    />
  {/if}
  {#if learningToday && workspace.backend.course && workspace.info && workspace.me?.id}
    <SidebarNavButton
      testId="open-learning-today"
      active={learningToday.active}
      label={m.learning_today_title()}
      onclick={learningToday.onOpen}
      icon={CalendarCheckIcon}
    />
  {/if}
  {#if timeHref && workspace.me?.id}
    <SidebarNavButton
      testId="open-my-time"
      active={meTime}
      label={m.time_title()}
      onclick={() => onOpenTime?.()}
      icon={ClockIcon}
    />
  {/if}
  {#if orgHref && !ownSpace}
    <a
      href={orgHref}
      data-testid="open-organization"
      class="flex min-h-8 w-full items-center gap-2 rounded-md px-2 text-left hover:bg-sidebar-accent roomy:min-h-touch"
    >
      <BuildingIcon class="size-5 shrink-0 p-0.5" aria-hidden="true" />
      <span class="min-w-0 flex-1 truncate">{m.shell_organization()}</span>
    </a>
  {/if}
  <button
    type="button"
    data-testid="open-settings"
    aria-label={m.shell_account_of({ name: myName })}
    aria-haspopup="dialog"
    class="flex min-h-8 w-full items-center gap-2 rounded-md px-2 text-left hover:bg-sidebar-accent roomy:min-h-touch"
    onclick={onOpenSettings}
  >
    <Avatar
      name={myName}
      colorKey={workspace.me?.id}
      locale={workspace.locale}
      decorative
      class="size-5"
    />
    <span class="min-w-0 flex-1 truncate">{myName}</span>
  </button>
</div>

<SidebarDialogs
  {workspace}
  {access}
  bind:peopleOpen
  {trash}
  bind:trashOpen
  {naming}
  bind:namingOpen
  onSubmitName={submitName}
  {asked}
  bind:askOpen
  onConfirmDelete={confirmed}
  {copyingContainer}
  bind:copyOpen
  onCopiedToWorkspace={onOpenContainer}
/>
