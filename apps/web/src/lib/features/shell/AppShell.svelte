<!--
  The frame of the workspace: the sidebar (`SidebarStrip` / `Sidebar`), the top
  bar (`TopBar`), and the pane (`children`), with dialogs in `ShellDialogs.svelte`.
-->
<script module lang="ts">
  import type { Workspace } from '$lib/state/index.ts';

  /** What the shell lends the page beside it. */
  export interface PaneTools {
    workspace: Workspace;
    /** Opens the picker of what to make; none for a guest, who makes nothing. */
    addBoard: (() => void) | undefined;
  }
</script>

<script lang="ts">
  import { tick, type Snippet } from 'svelte';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { ContainerPlace } from '$lib/logic/urls.ts';
  import type { Container } from '$lib/logic/wire.ts';
  import { closeOnBack } from '$lib/layout/back.svelte.ts';
  import { focusRevealed, Opener } from '$lib/focus.ts';
  import { sidebarShape } from '$lib/layout/presentation.ts';
  import { paneCovered } from '$lib/layout/over-pane.svelte.ts';
  import { page, pane } from '$lib/layout/room.svelte.ts';
  import {
    createCourse,
    createFromTemplate,
    createWiki,
    isGuest,
    newDiary,
    whileHere,
    type ContainerDraft,
    type NewKind,
  } from '$lib/state/index.ts';
  import type { AuditRecord } from '$lib/state/organization.ts';

  import { devReadout } from './dev.svelte.ts';
  import { shellLayout } from './layout.ts';
  import ShellDialogs from './ShellDialogs.svelte';
  import { SidebarState } from './sidebar-state.svelte.ts';
  import Sidebar from './Sidebar.svelte';
  import SidebarStrip from './SidebarStrip.svelte';
  import TopBar from './TopBar.svelte';
  import { undoKey } from './undo-keys.ts';

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
    sharedHref: (containerId: string, readToken: string) => string;
    onOpenContainer: (container: ContainerPlace) => void;
    locales?: readonly string[];
    language?: string;
    autoLocale?: string;
    onChooseLanguage?: (setting: string) => void;
    onSignOut?: () => void;
    onChangePassword?: (oldPassword: string, newPassword: string) => Promise<void>;
    accountSettings?: Snippet;
    settingsOpen?: boolean;
    orgHref?: string;
    onSwitchWorkspace?: (workspaceId: string) => void;
    onCreateOrganization?: (name: string) => Promise<void>;
    readRecords?: (workspaceId: string) => Promise<AuditRecord[]>;
    search?: Snippet<[Workspace]>;
    children: Snippet<[PaneTools]>;
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
    sharedHref,
    onOpenContainer,
    locales,
    language,
    autoLocale,
    onChooseLanguage,
    onSignOut,
    onChangePassword,
    accountSettings,
    settingsOpen = $bindable(false),
    orgHref,
    onSwitchWorkspace,
    onCreateOrganization,
    readRecords,
    search,
    children,
  }: Props = $props();

  devReadout.start(location.search);

  const sidebar = new SidebarState();
  const stage = $derived(page.stage);
  const layout = $derived(shellLayout(sidebarShape(stage), sidebar.collapsed));
  const drawerOut = $derived(layout.drawer && sidebar.drawerOpen);

  let shell = $state<HTMLElement | null>(null);
  let panel = $state<HTMLElement | null>(null);
  let recordsOpen = $state(false);
  const worksHere = $derived(workspace.workspaces.some((one) => one.id === workspace.info?.id));

  $effect(() => {
    if (!layout.drawer) sidebar.drawerOpen = false;
  });

  closeOnBack(
    () => drawerOut,
    () => {
      sidebar.drawerOpen = false;
    },
  );

  const opener = new Opener();
  $effect(() => {
    if (!drawerOut) return;
    focusRevealed(panel, { preventScroll: true });
    return () => {
      const from = opener.take();
      const lost = document.activeElement;
      if (!from) return;
      if (lost === null || lost === document.body || panel?.contains(lost)) focusRevealed(from);
    };
  });

  function openDrawer(event: Event): void {
    opener.remember(event);
    sidebar.drawerOpen = true;
  }

  async function focusOn(testId: string): Promise<void> {
    await tick();
    focusRevealed(shell?.querySelector<HTMLElement>(`[data-testid="${testId}"]`));
  }

  function expand(event: Event): void {
    if (layout.drawer) {
      openDrawer(event);
    } else {
      sidebar.setCollapsed(false);
      void focusOn('close-sidebar');
    }
  }

  function closeList(): void {
    if (layout.list === 'beside') {
      sidebar.setCollapsed(true);
      void focusOn('expand-sidebar');
    } else {
      sidebar.drawerOpen = false;
    }
  }

  function openContainer(container: ContainerPlace): void {
    onOpenContainer(container);
    sidebar.drawerOpen = false;
  }

  const closeDrawerThen = (action?: () => void) =>
    action ? () => (action(), (sidebar.drawerOpen = false), undefined) : undefined;
  const drawerEntry = (entry?: { active: boolean; onOpen: () => void }) =>
    entry && {
      active: entry.active,
      onOpen: () => void (entry.onOpen(), (sidebar.drawerOpen = false)),
    };
  const todayHubEntry = $derived(drawerEntry(todayHub));
  const reviewsEntry = $derived(drawerEntry(reviewsToday));
  const todayEntry = $derived(drawerEntry(learningToday));

  function openSettings(): void {
    if (!drawerOut) {
      settingsOpen = true;
      return;
    }
    const open = (): void => {
      window.removeEventListener('popstate', open);
      clearTimeout(fallback);
      settingsOpen = true;
    };
    const fallback = setTimeout(open, 2000);
    window.addEventListener('popstate', open);
    sidebar.drawerOpen = false;
  }

  let picking = $state(false);
  const addBoard = (): void => {
    picking = true;
  };
  /** Makes a new board, wiki, course or diary as the window of new things asked, and opens it. */
  async function addNew(kind: NewKind, draft: ContainerDraft): Promise<void> {
    const made = await whileHere(workspace, () => makeNew(kind, draft));
    if (made) openContainer(made);
  }
  function makeNew(kind: NewKind, draft: ContainerDraft): Promise<Container | undefined> {
    switch (kind) {
      case 'board':
        return workspace.createBoard(
          { ...draft, scaffold: true },
          m['View.NewBoardTitle'](),
          m.shell_new_board_status(),
        );
      case 'wiki':
        return createWiki(workspace, draft);
      case 'course':
        return createCourse(workspace, { ...draft, minimumRole: 'commenter' });
      case 'diary':
        return newDiary(workspace, draft.title);
    }
  }
  async function addFromTemplate(template: Container, draft: ContainerDraft): Promise<void> {
    const made = await whileHere(workspace, () =>
      createFromTemplate(workspace, template.id, draft),
    );
    if (made) openContainer(made);
  }

  function onkeydown(event: KeyboardEvent): void {
    if (event.key === 'Escape' && drawerOut && !event.defaultPrevented) {
      if (event.target instanceof Node && panel?.contains(event.target)) {
        event.preventDefault();
        sidebar.drawerOpen = false;
      }
      return;
    }
    const asked = undoKey(event);
    if (!asked) return;
    event.preventDefault();
    if (asked === 'undo') void workspace.undoLast();
    else void workspace.redoLast();
  }
</script>

<svelte:window {onkeydown} />

<div
  bind:this={shell}
  data-testid="app-shell"
  data-stage={stage}
  data-sidebar={layout.list === 'beside' ? 'fixed' : drawerOut ? 'drawer' : 'closed'}
  class="flex h-[calc(100dvh-var(--safe-top))] min-h-0 overflow-hidden"
>
  <SidebarStrip
    {workspace}
    {containerId}
    {meTime}
    {timeHref}
    onOpenTime={closeDrawerThen(onOpenTime)}
    todayHub={todayHubEntry}
    reviewsToday={reviewsEntry}
    learningToday={todayEntry}
    {containerHref}
    hidden={!layout.strip}
    inert={drawerOut || paneCovered()}
    onOpenContainer={openContainer}
    onExpand={expand}
    onAddBoard={addBoard}
    onOpenSettings={openSettings}
  />

  {#if drawerOut}
    <button
      type="button"
      tabindex="-1"
      data-testid="sidebar-scrim"
      aria-label={m.shell_close_sidebar()}
      class="fixed inset-0 z-40 animate-in cursor-default scrim duration-200 fade-in-0"
      onclick={() => (sidebar.drawerOpen = false)}
    ></button>
  {/if}

  <div
    bind:this={panel}
    data-testid="sidebar"
    role={drawerOut ? 'dialog' : undefined}
    aria-modal={drawerOut ? 'true' : undefined}
    aria-label={m.ui_sidebar_title()}
    tabindex="-1"
    inert={layout.list === 'over' && !drawerOut}
    class={[
      'flex flex-col bg-sidebar text-sidebar-foreground outline-none',
      layout.list === 'beside'
        ? 'w-64 shrink-0 border-r'
        : 'fixed inset-y-0 left-0 z-40 w-[calc(min(18rem,75vw)+var(--safe-left))] border-r pt-(--safe-top) pl-(--safe-left) shadow-lg',
      layout.list === 'over' &&
        (drawerOut
          ? '[transition:translate_200ms_ease-out]'
          : 'invisible -translate-x-full [transition:translate_200ms_ease-in,visibility_0s_200ms]'),
    ]}
  >
    <Sidebar
      {workspace}
      {containerId}
      {meTime}
      {timeHref}
      onOpenTime={closeDrawerThen(onOpenTime)}
      todayHub={todayHubEntry}
      reviewsToday={reviewsEntry}
      learningToday={todayEntry}
      {containerHref}
      beside={layout.list === 'beside'}
      onOpenContainer={openContainer}
      onClose={closeList}
      onAddBoard={addBoard}
      onOpenSettings={openSettings}
      {orgHref}
      {onSwitchWorkspace}
      {onCreateOrganization}
    />
  </div>

  <main
    class="@container/pane flex min-w-0 flex-1 flex-col outline-none"
    tabindex="-1"
    inert={drawerOut}
    {@attach pane.attach}
  >
    <TopBar
      {workspace}
      {containerId}
      {meTime}
      todayHub={todayHub?.active === true}
      reviewsToday={reviewsToday?.active === true}
      learningToday={learningToday?.active === true}
      {sharedHref}
      menuButton={layout.menuButton}
      onOpenSidebar={openDrawer}
      {search}
      inert={paneCovered()}
    />
    <div class="relative min-h-0 flex-1">
      {@render children({ workspace, addBoard: isGuest(workspace) ? undefined : addBoard })}
    </div>
  </main>
</div>

<ShellDialogs
  {workspace}
  bind:picking
  bind:settingsOpen
  bind:recordsOpen
  {worksHere}
  {locales}
  {language}
  {autoLocale}
  {onChooseLanguage}
  {onSignOut}
  {onChangePassword}
  {accountSettings}
  {containerHref}
  onOpenContainer={openContainer}
  onCreate={(kind: NewKind, draft: ContainerDraft) => void addNew(kind, draft)}
  onFromTemplate={(template: Container, draft: ContainerDraft) =>
    void addFromTemplate(template, draft)}
  {readRecords}
/>
