<!--
  The header of a view: which view is shown, and what is done to it (which
  properties its cards show, grouping, filter, sort, search, a new card).

  The views are always behind one button at its start, which names the view
  shown and opens the list of them all, and of what is done to a view (add,
  rename, copy, delete: ViewSwitcher.svelte). It looks at the width of the
  pane it is in, not of the window (`pane.row`). With room for all of it, it
  is one row: that button, the tools as icons, "New". In a narrower pane it
  folds into one row of a different make: that button, search and filter as
  icons, the rest behind "…", and "New" as a round button at the bottom of
  the board. A tool is an icon in both rows: its name is a tooltip and what
  a screen reader hears, and what it has on (properties shown, conditions,
  sorts) a count.

  The two are one component, and every tool is one element in both. A tool
  that has no button of its own (in the folded row all but search and filter;
  "dependencies" in neither) keeps its element, unseen, on "…": what it opened stays
  open while the pane changes its width, and stays next to the button it was
  opened from. Whether a tool opens as a popover or as a sheet is not decided
  here (lib/layout/presentation.ts).

  A reader who may not edit the board can switch views and search. The
  other tools change the view for everyone and are not shown.

  "…" also saves the view as a CSV file (view-csv.ts), for every reader but
  one who came by a public link: in both rows, after the tools it holds.

  The view shown is said to a screen reader as it changes (a quiet status
  with its name): chosen from the list, added, copied, or reached by the
  browser's back.
-->
<script lang="ts">
  import ArrowUpDownIcon from '@lucide/svelte/icons/arrow-up-down';
  import CalendarDaysIcon from '@lucide/svelte/icons/calendar-days';
  import EllipsisIcon from '@lucide/svelte/icons/ellipsis';
  import EyeIcon from '@lucide/svelte/icons/eye';
  import FileSpreadsheetIcon from '@lucide/svelte/icons/file-spreadsheet';
  import GitForkIcon from '@lucide/svelte/icons/git-fork';
  import LayersIcon from '@lucide/svelte/icons/layers';
  import ListFilterIcon from '@lucide/svelte/icons/list-filter';
  import PlusIcon from '@lucide/svelte/icons/plus';
  import { tick, untrack } from 'svelte';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import ActionMenu from '$lib/components/menu/ActionMenu.svelte';
  import type { MenuAction } from '$lib/components/menu/actions.ts';
  import { PickerPresentation } from '$lib/components/picker-surface/default-presentation.svelte.ts';
  import PickerSurface from '$lib/components/picker-surface/PickerSurface.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { pane } from '$lib/layout/room.svelte.ts';
  import type { BoardView, Card, PropertyTemplate } from '$lib/logic/wire.ts';
  import type { BoardStore } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import ChoiceTool from './ChoiceTool.svelte';
  import { conditionCount } from './filter-edit.ts';
  import FilterEditor from './FilterEditor.svelte';
  import PropertiesPanel from './PropertiesPanel.svelte';
  import SortEditor from './SortEditor.svelte';
  import { asideTools, viewTitle, type Icon } from './texts.ts';
  import { saveViewCsv } from './view-csv.ts';
  import {
    dependenciesChoices,
    dependenciesName,
    dependenciesOffered,
    displayByChoices,
    displayByName,
    displayByOffered,
    groupableProperties,
    groupingFields,
    groupingOf,
    groupProperty,
    propertyToggles,
    sortOffered,
    type DisplayByChoice,
  } from './view-fields.ts';
  import ViewSearch from './ViewSearch.svelte';
  import ViewSwitcher from './ViewSwitcher.svelte';

  interface Props {
    board: BoardStore;
    view: BoardView;
    /** The search text of the view. Bindable. */
    search: string;
    viewHref: (viewId: string) => string;
    onOpenView: (viewId: string) => void;
    /** Makes a card the view will show, and opens it. */
    onAddCard: () => void;
    /** The cards the view shows now, for a CSV of it; not given on a public link. */
    exportCards?: (() => readonly Card[]) | undefined;
  }

  let {
    board,
    view,
    search = $bindable(),
    viewHref,
    onOpenView,
    onAddCard,
    exportCards,
  }: Props = $props();

  const pickers = new PickerPresentation();

  /** The pane has room for the whole header in one row. */
  const row = $derived(pane.row);
  const templates = $derived(board.board?.cardProperties ?? []);
  const canEdit = $derived(board.canEdit);
  const grouping = $derived(groupingOf(view.fields.viewType));
  const sorting = $derived(sortOffered(view.fields.viewType));
  const groupedBy = $derived(groupProperty(view, templates));
  const groupName = $derived(groupedBy?.name ?? m.viewheader_group_none());
  /** A calendar or a Gantt chart: the date its cards are placed by; when they were made, where the board has none. */
  const displayBy = $derived(displayByOffered(view.fields.viewType));
  const placedByName = $derived(displayByName(view, templates, m['PropertyType.CreatedTime']()));
  /** A Gantt chart: the card relation it reads as what comes before what. */
  const dependencies = $derived(dependenciesOffered(view.fields.viewType));
  const dependsOnName = $derived(dependenciesName(view, templates, m.viewheader_group_none()));
  const filterCount = $derived(conditionCount(view.fields.filter));
  const sortCount = $derived(view.fields.sort.length);
  /** The properties the cards show, of those the panel can switch. */
  const shownCount = $derived(
    propertyToggles(templates, view.fields.viewType).filter((toggle) =>
      view.fields.shownProperties.includes(toggle.id),
    ).length,
  );

  let header = $state<HTMLElement | null>(null);

  // What is open. Kept here, above the surfaces, so that it outlives a
  // surface that is rebuilt when a sheet becomes a popover.
  let propertiesOpen = $state(false);
  let groupOpen = $state(false);
  let displayOpen = $state(false);
  let dependenciesOpen = $derived<boolean>(dependencies && false); // closed when the tool goes
  let filterOpen = $state(false);
  let sortOpen = $state(false);

  /**
   * After a tool without a button of its own closes (in the folded row; with
   * `always`, in either), the focus goes to "…", where it was opened from.
   */
  function closedAside(open: boolean, always = false): void {
    if (open || (row && !always)) return;
    void tick().then(() => header?.querySelector<HTMLElement>('[data-tool="more"]')?.focus());
  }

  // ------------------------------------------------------------ status
  const shownId = $derived(view.id);
  /** The name the status says, read when another view is shown: a view renamed is not another. */
  const shownName = $derived.by(() => {
    const id = shownId;
    return untrack(() => viewTitle(board.view(id) ?? view));
  });

  // ------------------------------------------------------------ search

  /** The view whose search field was opened (ViewSearch.svelte). Another view starts closed. */
  let searchOpenIn = $state<string | undefined>(undefined);
  const searchShown = $derived(searchOpenIn === view.id || search !== '');
  /** The folded row shows the search field in place of everything else. */
  const searchBar = $derived(!row && searchShown);

  // ---------------------------------------------------------- grouping

  function groupBy(template: PropertyTemplate | undefined): void {
    if ((template?.id ?? '') === (view.fields.groupBy ?? '')) return;
    // One change, so that one undo brings back the grouping and its columns.
    void board.setViewFields(view.id, groupingFields(template), 'groupBy');
  }

  const groupActions = $derived<MenuAction[]>([
    ...(grouping.optional
      ? [
          {
            id: 'none',
            label: m['GroupBy.ungroup'](),
            checked: groupedBy === undefined,
            run: () => {
              groupBy(undefined);
            },
          },
        ]
      : []),
    ...groupableProperties(templates).map((template, at) => ({
      id: template.id,
      label: template.name,
      checked: template.id === groupedBy?.id,
      separated: grouping.optional && at === 0,
      run: () => {
        groupBy(template);
      },
    })),
  ]);

  // --------------------------------------- display by, and dependencies

  /** The lines of a list of choices: choosing one that changes the view writes it. */
  function choiceActions(
    choices: readonly DisplayByChoice[],
    member: 'dateProperty' | 'dependencyPropertyId',
  ): MenuAction[] {
    return choices.map(({ write, ...line }) => ({
      ...line,
      // The line of "none" has the empty identifier, which names nothing in the markup.
      id: line.id || '(none)',
      run: () => {
        if (write) void board.setViewFields(view.id, { [member]: line.id }, member);
      },
    }));
  }
  const displayActions = $derived(
    choiceActions(displayByChoices(view, templates, placedByName), 'dateProperty'),
  );
  const dependenciesActions = $derived(
    choiceActions(
      dependenciesChoices(view, templates, m.viewheader_group_none()),
      'dependencyPropertyId',
    ),
  );

  // ------------------------------------------------------------- "…"

  /** The lines of "…" for the tools that have no button of their own in the row as it is. */
  const asideActions = $derived(
    asideTools(
      {
        row,
        sort: sorting ? sortCount : undefined,
        displayBy: displayBy ? placedByName : undefined,
        dependencies: dependencies ? dependsOnName : undefined,
        group: grouping.offered ? groupName : undefined,
      },
      {
        sort: () => void (sortOpen = true),
        'display-by': () => void (displayOpen = true),
        dependencies: () => void (dependenciesOpen = true),
        group: () => void (groupOpen = true),
        properties: () => void (propertiesOpen = true),
      },
    ),
  );

  /** The lines of "…": the tools it holds for an editor, then the file of the view. */
  const moreActions = $derived.by<MenuAction[]>(() => {
    const tools = canEdit ? asideActions : [];
    if (!exportCards) return tools;
    const cards = exportCards;
    const save: MenuAction = {
      id: 'export-csv',
      label: m['ViewHeader.export-csv'](),
      icon: FileSpreadsheetIcon,
      separated: tools.length > 0,
      run: () => {
        saveViewCsv(board, view, cards());
      },
    };
    return [...tools, save];
  });

  /** A tool that is not drawn: an unseen element where "…" is in the row, for its popover to open at. */
  const ASIDE = 'invisible absolute top-1/2 size-8 -translate-y-1/2 roomy:size-touch';
  const asideAt = $derived([ASIDE, row ? 'right-26' : 'right-1.5']);
</script>

<!--
  The button of a tool: its icon and a count, its name as a tooltip; or
  (`aside`) nothing to see in the folded row; `always`: nothing to see in
  either row.
-->
{#snippet tool(
  props: Record<string, unknown>,
  id: string,
  ToolIcon: Icon,
  label: string,
  count: number,
  aside: boolean | 'always',
)}
  <Button
    {...props}
    variant="ghost"
    data-tool={id}
    aria-label={count > 0 ? `${label} (${count})` : label}
    title={count > 0 ? `${label} (${count})` : label}
    class={[
      'shrink-0 roomy:h-touch',
      aside === 'always' || (aside && !row) ? asideAt : 'gap-1 px-2 roomy:min-w-touch',
    ]}
  >
    <ToolIcon />
    {#if count > 0}
      <span
        class="inline-flex h-4.5 min-w-4.5 items-center justify-center rounded-full bg-primary px-1 text-xs leading-none font-semibold text-primary-foreground"
        data-count
      >
        {count}
      </span>
    {/if}
  </Button>
{/snippet}

<div
  bind:this={header}
  data-testid="view-header"
  data-layout={row ? 'row' : 'folded'}
  class={['relative flex h-12 shrink-0 items-center gap-1 border-b', row ? 'px-4' : 'px-1.5']}
>
  <div class="contents" inert={searchBar}>
    <nav aria-label={m.viewheader_views()} class="flex min-w-0 shrink">
      <ViewSwitcher {board} {view} {viewHref} {onOpenView} />
    </nav>
    <div class="min-w-1 flex-1"></div>
  </div>
  <ViewSearch bind:search bind:openIn={searchOpenIn} viewId={view.id} {row} />
  <div class="contents" inert={searchBar}>
    {#if canEdit}
      <PickerSurface
        bind:open={
          () => propertiesOpen,
          (open: boolean) => {
            propertiesOpen = open;
            closedAside(open);
          }
        }
        presentation={pickers.current}
        title={m['ViewHeader.properties']()}
      >
        {#snippet trigger({ props }: { props: Record<string, unknown> })}
          {@const word = m['ViewHeader.properties']()}
          {@render tool(props, 'properties', EyeIcon, word, shownCount, true)}
        {/snippet}
        <PropertiesPanel {board} {view} />
      </PickerSurface>

      {#if grouping.offered}
        <ChoiceTool
          bind:open={groupOpen}
          id="group"
          icon={LayersIcon}
          title={m.viewheader_group_by()}
          subtitle={viewTitle(view)}
          word={m['ViewHeader.group-by']({ property: groupName })}
          actions={groupActions}
          {tool}
          onchange={closedAside}
        />
      {/if}

      {#if displayBy}
        <ChoiceTool
          bind:open={displayOpen}
          id="display-by"
          icon={CalendarDaysIcon}
          title={m.viewheader_display_by()}
          subtitle={viewTitle(view)}
          word={m['ViewHeader.display-by']({ property: placedByName })}
          actions={displayActions}
          {tool}
          onchange={closedAside}
        />
      {/if}

      {#if dependencies}
        <ChoiceTool
          bind:open={dependenciesOpen}
          id="dependencies"
          icon={GitForkIcon}
          title={m.viewheader_dependencies()}
          subtitle={viewTitle(view)}
          word={m.viewheader_dependencies()}
          actions={dependenciesActions}
          aside="always"
          {tool}
          onchange={(open: boolean) => {
            closedAside(open, true);
          }}
        />
      {/if}

      <PickerSurface
        bind:open={filterOpen}
        presentation={pickers.current}
        title={m['ViewHeader.filter']()}
      >
        {#snippet trigger({ props }: { props: Record<string, unknown> })}
          {@const word = m['ViewHeader.filter']()}
          {@render tool(props, 'filter', ListFilterIcon, word, filterCount, false)}
        {/snippet}
        <FilterEditor {board} {view} />
      </PickerSurface>

      {#if sorting}
        <PickerSurface
          bind:open={
            () => sortOpen,
            (open: boolean) => {
              sortOpen = open;
              closedAside(open);
            }
          }
          presentation={pickers.current}
          title={m['ViewHeader.sort']()}
        >
          {#snippet trigger({ props }: { props: Record<string, unknown> })}
            {@const word = m['ViewHeader.sort']()}
            {@render tool(props, 'sort', ArrowUpDownIcon, word, sortCount, true)}
          {/snippet}
          <SortEditor {board} {view} />
        </PickerSurface>
      {/if}
    {/if}
  </div>

  <div class="contents" inert={searchBar}>
    <!-- The tools without a button of their own in the row as it is (none, but in a Gantt chart, in the whole row), and the file of the view. -->
    {#if moreActions.length > 0}
      <ActionMenu
        title={m['ViewHeader.view-menu']()}
        subtitle={viewTitle(view)}
        actions={moreActions}
      >
        {#snippet trigger(props: Record<string, unknown>)}
          <IconButton {...props} label={m['ViewHeader.view-menu']()} data-tool="more">
            <EllipsisIcon />
          </IconButton>
        {/snippet}
      </ActionMenu>
    {/if}
    {#if canEdit && row}
      <Button class="ml-1 shrink-0 roomy:h-touch" data-testid="new-card" onclick={onAddCard}>
        <PlusIcon data-icon="inline-start" />
        {m['ViewHeader.new']()}
      </Button>
    {/if}
  </div>
</div>

{#if canEdit && !row}
  <!--
    "New" in the folded layout: a round button at the bottom of the board,
    where a thumb is. Placed against the box of the board page; it stays
    clear of the home indicator and rides on top of the on-screen keyboard.
    The lists of the views end `--fab-room` (the board page's) below their
    last card, which then scrolls clear of it; a table's last rows scroll
    above its summary row, which stays at the bottom.
  -->
  <Button
    aria-label={m['ViewHeader.new']()}
    title={m['ViewHeader.new']()}
    data-testid="new-card"
    data-round
    class="absolute right-4 bottom-[calc(1rem+var(--safe-bottom)+var(--keyboard))] z-20 size-14 rounded-full shadow-lg"
    onclick={onAddCard}
  >
    <PlusIcon class="size-6" />
  </Button>
{/if}

<!-- Outside the header: in the folded row an open search field leaves the rest of it out of reach. -->
<!-- Each view shown is a new line in it, said also where its name is the one said before. -->
<p class="sr-only" role="status" data-testid="view-shown">
  {#key shownId}<span>{m.viewheader_view_shown({ name: shownName })}</span>{/key}
</p>
