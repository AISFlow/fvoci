<!--
  The pages of a wiki or the chapters and units of a course as a tree
  (`role="tree"`), and making nodes.

  One item of the tree is in the tab order at a time (the shown page/unit, or
  the last one moved to); keyboard navigation follows the ARIA tree pattern.
  Editors can drag nodes (`tree-drag.svelte.ts`), move with Alt+Arrow keys
  (respecting `rules`, such as course depth-2 and chapter-only parenting), or
  use the row menu.
-->
<script lang="ts" generics="P extends TreeItem">
  import { tick, type Snippet } from 'svelte';

  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
  import EllipsisIcon from '@lucide/svelte/icons/ellipsis';
  import PlusIcon from '@lucide/svelte/icons/plus';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import ActionMenu from '$lib/components/menu/ActionMenu.svelte';
  import type { MenuAction } from '$lib/components/menu/actions.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { isApplePlatform } from '$lib/keys.ts';
  import { fingerPointer } from '$lib/layout/room.svelte.ts';
  import { stepped, type MoveStep, type PagePlace, type TreeRules } from '$lib/logic/page-moves.ts';
  import { pageRows, type PageRow } from '$lib/logic/page-tree.ts';
  import type { PageDraft, TreeItem, TreeSource } from '$lib/state/index.ts';

  import { viewersOf } from './page-viewers.ts';
  import PageViewers from './PageViewers.svelte';
  import { buildTreeActions, WIKI_TREE_LABELS, type TreeLabels } from './tree-actions.ts';
  import { TreeDrag } from './tree-drag.svelte.ts';
  import type { TreeOpen } from './tree-open.svelte.ts';

  interface Props {
    wiki: TreeSource<P>;
    /** The page or unit currently shown. */
    currentId: string | undefined;
    open: TreeOpen;
    pageHref: (pageId: string) => string;
    onOpenPage: (pageId: string) => void;
    /** Makes a page or unit as the draft says, and shows it. */
    onCreate: (draft: PageDraft) => void;
    /** Asks to rename a chapter (courses only). */
    onRename?: ((page: P) => void) | undefined;
    /** Asks to delete a node; omitted where deletion is unavailable. */
    onDelete?: ((page: P) => void) | undefined;
    /** Moves a node with its descendants; omitted where moving is unavailable. */
    onMove?: ((pageId: string, to: PagePlace) => Promise<unknown>) | undefined;
    /** Asks where to move a node ("Move…"). */
    onMoveTo?: ((page: P) => void) | undefined;
    /** Copies a page with its descendants (wikis only). */
    onDuplicate?: ((page: P) => Promise<string | undefined>) | undefined;
    /** Placement rules (e.g. course depth-2 and chapter-only parenting). */
    rules?: TreeRules<P> | undefined;
    /** Wordings; defaults to wiki pages. */
    labels?: TreeLabels | undefined;
    /** Optional header actions replacing the default "+" button (e.g. "+ Unit" and "+ Chapter"). */
    headerActions?: Snippet | undefined;
    /** Optional leading indicator rendered inside each non-chapter row (e.g. course completion check). */
    itemLeading?: Snippet<[P]> | undefined;
    /** Optional trailing indicator rendered inside each non-chapter row (e.g. study amount/date badge). */
    itemTrailing?: Snippet<[P]> | undefined;
    /** Root `data-testid`; defaults to `page-tree` for wikis. */
    testId?: string;
    /** A creation is under way: no other is asked for meanwhile. */
    busy?: boolean;
  }

  let {
    wiki,
    currentId,
    open,
    pageHref,
    onOpenPage,
    onCreate,
    onRename,
    onDelete,
    onMove,
    onMoveTo,
    onDuplicate,
    rules,
    labels = WIKI_TREE_LABELS,
    headerActions,
    itemLeading,
    itemTrailing,
    testId = 'page-tree',
    busy = false,
  }: Props = $props();

  const ALT_STEPS: Partial<Record<string, MoveStep>> = {
    ArrowUp: 'up',
    ArrowDown: 'down',
    ArrowRight: 'in',
    ArrowLeft: 'out',
  };
  const alt = isApplePlatform() ? '⌥' : 'Alt+';
  const keys = $derived(!fingerPointer.current);
  let said = $state('');

  const uid = $props.id();
  const rows = $derived(pageRows(wiki.tree, (pageId) => open.has(pageId)));
  let moved = $derived<string | undefined>(currentId);
  const active = $derived(
    [moved, currentId, rows[0]?.page.id].find(
      (pageId) => pageId !== undefined && rows.some((row) => row.page.id === pageId),
    ),
  );
  let menuOf = $state<string | undefined>(undefined);
  const mayMove = $derived(wiki.canEdit && onMove !== undefined);
  const drag = new TreeDrag<P>({
    rows: () => rows,
    pages: () => wiki.pages,
    enabled: () => mayMove,
    rules: () => rules,
    unfold: (pageId) => {
      open.set([pageId]);
    },
    ondrop: (pageId, to) => {
      void onMove?.(pageId, to);
    },
  });

  const titleOf = (page: P): string => page.title || labels.untitled(page);
  const rowId = (pageId: string): string => `${uid}-${pageId}`;
  const draggedTitle = $derived.by(() => {
    const page = wiki.page(drag.dragged);
    return page ? titleOf(page) : '';
  });

  /** Moves a node a step (Alt+Arrow or menu); false when the step goes nowhere. */
  function moveBy(pageId: string, step: MoveStep): boolean {
    const to = stepped(wiki.pages, pageId, step, rules);
    if (!onMove || !to) return false;
    if (to.parentId !== '') open.set([to.parentId]);
    const at = rows.findIndex((row) => row.page.id === pageId);
    const moving = onMove(pageId, to);
    void focusOn(pageId);
    void moving.then(() => {
      const row = rows.find((one) => one.page.id === pageId);
      const here = row ? pageId : (rows[at] ?? rows.at(-1))?.page.id;
      if (document.activeElement === document.body) void focusOn(here);
      if (row) {
        said = m.wiki_moved_to({
          title: titleOf(row.page),
          position: row.position,
          count: row.siblings,
          level: row.depth + 1,
        });
      }
    });
    return true;
  }

  async function focusOn(pageId: string | undefined): Promise<void> {
    if (pageId === undefined) return;
    moved = pageId;
    await tick();
    document.getElementById(rowId(pageId))?.focus();
  }

  function onkeydown(event: KeyboardEvent): void {
    if (event.defaultPrevented || !(event.target instanceof HTMLElement)) return;
    if (event.target.getAttribute('role') !== 'treeitem') return;
    const at = rows.findIndex((row) => row.page.id === active);
    const row = rows[at];
    if (!row) return;
    const altStep = ALT_STEPS[event.key];
    if (mayMove && altStep && event.altKey && !event.ctrlKey && !event.metaKey && !event.shiftKey) {
      event.preventDefault();
      moveBy(row.page.id, altStep);
      return;
    }
    if (event.altKey || event.ctrlKey || event.metaKey) return;
    const step = (to: PageRow<P> | undefined): void => {
      event.preventDefault();
      void focusOn(to?.page.id);
    };
    if (wiki.canEdit && (event.key === 'ContextMenu' || (event.key === 'F10' && event.shiftKey))) {
      event.preventDefault();
      menuOf = row.page.id;
      return;
    }
    if (event.shiftKey) return;
    switch (event.key) {
      case 'ArrowDown':
        step(rows[at + 1]);
        break;
      case 'ArrowUp':
        step(rows[at - 1]);
        break;
      case 'Home':
        step(rows[0]);
        break;
      case 'End':
        step(rows.at(-1));
        break;
      case 'ArrowRight':
        if (!row.hasChildren) break;
        if (row.open) step(rows[at + 1]);
        else {
          event.preventDefault();
          open.set([row.page.id]);
        }
        break;
      case 'ArrowLeft':
        if (row.open) {
          event.preventDefault();
          open.set([row.page.id], false);
        } else step(rows.find((one) => one.page.id === row.parentId));
        break;
      case 'Enter':
        event.preventDefault();
        if (row.page.kind === 'chapter') {
          if (row.hasChildren) open.set([row.page.id], !row.open);
        } else {
          onOpenPage(row.page.id);
        }
        break;
      default:
        break;
    }
  }

  function onclick(event: MouseEvent, pageId: string): void {
    moved = pageId;
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) {
      return;
    }
    event.preventDefault();
    onOpenPage(pageId);
  }

  function actionsOf(row: PageRow<P>): MenuAction[] {
    return buildTreeActions({
      row,
      labels,
      rules,
      canEdit: wiki.canEdit,
      mayMove,
      keys,
      alt,
      unfold: (id) => {
        open.set([id]);
      },
      onCreate,
      onRename,
      onMoveBy: (id, step) => {
        moveBy(id, step);
      },
      onMoveTo,
      onDuplicate,
      onCopied: (title) => {
        said = m.wiki_page_copied({ title });
      },
      onDelete,
    });
  }
</script>

<div
  class="flex min-h-0 flex-1 flex-col"
  data-testid={testId}
  data-settling={wiki.settling ? '' : undefined}
>
  <div class="flex shrink-0 items-center gap-1 px-3 pt-3 pb-1">
    <h2 id="{uid}-title" class="min-w-0 flex-1 truncate text-xs font-medium text-muted-foreground">
      {labels.heading()}
    </h2>
    {#if wiki.canEdit}
      {#if headerActions}
        {@render headerActions()}
      {:else}
        <IconButton
          label={m.wiki_new_page()}
          data-testid="new-page"
          disabled={busy}
          class="text-muted-foreground"
          onclick={() => {
            onCreate({});
          }}
        >
          <PlusIcon />
        </IconButton>
      {/if}
    {/if}
  </div>
  {#if rows.length > 0}
    <ul
      role="tree"
      aria-labelledby="{uid}-title"
      class={[
        'min-h-0 flex-1 overflow-y-auto overscroll-contain px-2 pb-3',
        drag.dragged && 'select-none',
      ]}
      {onkeydown}
      {@attach drag.attach}
    >
      {#each rows as row (row.page.id)}
        {@const page = row.page}
        {@const title = titleOf(page)}
        {@const isChapter = page.kind === 'chapter'}
        {@const mark = drag.mark?.pageId === page.id ? drag.mark : undefined}
        <li
          id={rowId(page.id)}
          role="treeitem"
          aria-label={title}
          aria-describedby={!isChapter && viewersOf(wiki, page.id).length > 0
            ? `${rowId(page.id)}-viewers`
            : undefined}
          aria-level={row.depth + 1}
          aria-setsize={row.siblings}
          aria-posinset={row.position}
          aria-expanded={row.hasChildren ? row.open : undefined}
          aria-selected={page.id === currentId}
          tabindex={page.id === active ? 0 : -1}
          data-page={page.id}
          data-kind={page.kind}
          data-drop={mark?.zone}
          aria-keyshortcuts={mayMove
            ? 'Alt+ArrowUp Alt+ArrowDown Alt+ArrowLeft Alt+ArrowRight'
            : undefined}
          class={[
            'group/page relative rounded-md outline-none focus-visible:ring-2 focus-visible:ring-ring',
            mayMove && 'select-none [-webkit-touch-callout:none]',
            drag.dragged === page.id && 'opacity-50',
          ]}
          onfocus={(event: FocusEvent) => {
            if (event.target === event.currentTarget) moved = page.id;
          }}
        >
          <div
            class={[
              'flex min-h-8 items-center gap-1 rounded-md pr-9 hover:bg-muted roomy:min-h-touch roomy:pr-12',
              isChapter && 'font-semibold text-foreground',
              page.id === currentId && 'bg-muted font-medium',
              mark?.zone === 'inside' && 'bg-primary/10 ring-2 ring-primary ring-inset',
            ]}
            style:padding-left="{0.25 + row.depth * 1}rem"
          >
            {#if row.hasChildren}
              <button
                type="button"
                tabindex="-1"
                aria-label={row.open ? m.wiki_fold({ title }) : m.wiki_unfold({ title })}
                class="grid size-6 shrink-0 place-items-center rounded-sm text-muted-foreground hover:bg-accent roomy:size-9"
                onclick={() => {
                  open.set([page.id], !row.open);
                }}
              >
                <ChevronRightIcon
                  class={['size-4 transition-transform', row.open && 'rotate-90']}
                  aria-hidden="true"
                />
              </button>
            {:else if isChapter}
              <span
                class="grid size-6 shrink-0 place-items-center text-muted-foreground/50 roomy:size-9"
                aria-hidden="true"
              >
                <ChevronRightIcon class="size-4" />
              </span>
            {:else}
              <span class="size-6 shrink-0 roomy:size-9" aria-hidden="true"></span>
            {/if}
            {#if isChapter}
              <button
                type="button"
                tabindex="-1"
                data-drag-handle
                class="flex min-w-0 flex-1 items-center gap-1.5 self-stretch py-1 text-left"
                onclick={() => {
                  moved = page.id;
                  if (row.hasChildren) open.set([page.id], !row.open);
                }}
              >
                <span class={['min-w-0 truncate text-sm', !page.title && 'text-muted-foreground']}>
                  {title}
                </span>
              </button>
            {:else}
              <a
                href={pageHref(page.id)}
                tabindex="-1"
                draggable="false"
                class="flex min-w-0 flex-1 items-center gap-1.5 self-stretch py-1"
                onclick={(event: MouseEvent) => {
                  onclick(event, page.id);
                }}
              >
                {@render itemLeading?.(page)}
                {#if page.icon}
                  <span class="shrink-0" aria-hidden="true" data-testid="tree-icon">
                    {page.icon}
                  </span>
                {/if}
                <span class={['min-w-0 flex-1 truncate', !page.title && 'text-muted-foreground']}>
                  {title}
                </span>
                {@render itemTrailing?.(page)}
              </a>
              <PageViewers
                {wiki}
                pageId={page.id}
                of={labels.viewersOf ?? 'page'}
                id="{rowId(page.id)}-viewers"
              />
            {/if}
          </div>
          {#if mark && mark.zone !== 'inside'}
            <div
              class={[
                'pointer-events-none absolute right-1 h-0.5 rounded-full bg-primary',
                mark.zone === 'before' ? '-top-px' : '-bottom-px',
              ]}
              style:left="{0.25 + mark.depth * 1}rem"
              data-testid="drop-line"
              aria-hidden="true"
            ></div>
          {/if}
          {#if wiki.canEdit}
            <ActionMenu
              bind:open={
                () => menuOf === page.id,
                (shown: boolean) => {
                  menuOf = shown ? page.id : undefined;
                  if (!shown) void focusOn(page.id);
                }
              }
              {title}
              actions={actionsOf(row)}
            >
              {#snippet trigger(props: Record<string, unknown>)}
                <IconButton
                  {...props}
                  tabindex={-1}
                  label={labels.menuLabel(page, title)}
                  class="absolute top-0 right-0 text-muted-foreground can-hover:opacity-0 can-hover:group-focus-within/page:opacity-100 can-hover:group-hover/page:opacity-100 can-hover:aria-expanded:opacity-100"
                >
                  <EllipsisIcon />
                </IconButton>
              {/snippet}
            </ActionMenu>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
  <p class="sr-only" aria-live="polite">{said}</p>
  {#if drag.dragged && drag.at}
    <div
      class="pointer-events-none fixed z-50 max-w-64 truncate rounded-md border bg-popover px-2 py-1 text-sm shadow-md"
      style:left="{drag.at.x + 12}px"
      style:top="{drag.at.y + 8}px"
      data-testid="drag-chip"
      aria-hidden="true"
    >
      {draggedTitle}
    </div>
  {/if}
</div>
