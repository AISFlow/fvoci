<!--
  Shared read-only tree & body reader for public `/shared/<container>/<item>?r=<token>`
  links of wikis (`kind="wiki"`) and courses (`kind="course"`): renders the
  sidebar/collapsible outline tree, heading outline (`PageOutline`), internal
  same-container references, and anonymous presence without comments, versions,
  or progress/schedule calls.
-->
<script lang="ts">
  import ListTreeIcon from '@lucide/svelte/icons/list-tree';

  import type { DocNode } from '$lib/editor/schema.ts';
  import BodyView from '$lib/editor/BodyView.svelte';
  import type { ReferenceDrawing } from '$lib/editor/reference-chip.ts';
  import OutlineButton from '$lib/features/wiki/OutlineButton.svelte';
  import OutlineColumns from '$lib/features/wiki/OutlineColumns.svelte';
  import { PageOutline } from '$lib/features/wiki/page-outline.svelte.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { pane } from '$lib/layout/room.svelte.ts';
  import { nameWindow } from '$lib/layout/window-title.svelte.ts';
  import { firstPage, firstUnit, pageRows, pageTree } from '$lib/logic/page-tree.ts';
  import { isBackendError, type Backend } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import NotFound from './NotFound.svelte';
  import type { Router } from './router.svelte.ts';

  interface SharedNode {
    readonly id: string;
    readonly parentId: string;
    readonly title: string;
    readonly icon?: string | undefined;
    readonly kind?: 'unit' | 'chapter' | undefined;
    readonly order: string;
  }

  interface SharedDetail {
    readonly id: string;
    readonly title: string;
    readonly icon?: string | undefined;
    readonly body: DocNode | null;
  }

  interface Props {
    kind: 'wiki' | 'course';
    backend: Backend;
    containerId: string;
    /** What the window is named after, below the item shown. */
    containerTitle?: string | undefined;
    router: Router;
  }

  let { kind, backend, containerId, containerTitle, router }: Props = $props();

  let status = $state<'loading' | 'ready' | 'failed'>('loading');
  let items = $state.raw<SharedNode[]>([]);
  let shown = $state.raw<SharedDetail | null | undefined>(undefined);
  let itemFailed = $state(false);
  let treeOpen = $state(false);
  let treeReadings = 0;
  let itemReadings = 0;
  const uid = $props.id();

  const itemId = $derived(router.route.kind === 'shared' ? router.route.viewId : undefined);
  const rows = $derived(pageRows(pageTree(items), () => true));
  const hasContentItems = $derived(
    kind === 'course' ? items.some((item) => item.kind !== 'chapter') : items.length > 0,
  );
  const narrow = $derived(pane.measured && !pane.columns);
  const outline = new PageOutline();
  const hrefOf = (id: string): string =>
    router.href({ ...router.route, viewId: id, cardId: undefined });

  const references: ReferenceDrawing = {
    shown: (id) => {
      const target = items.find((one) => one.id === id && (kind === 'wiki' || one.kind === 'unit'));
      if (!target) return null;
      return {
        title: target.title,
        icon: kind === 'wiki' ? (target.icon ?? '') : '',
        kind: kind === 'wiki' ? 'page' : 'unit',
        href: hrefOf(id),
      };
    },
    open: (id) => {
      if (kind === 'wiki' || items.some((one) => one.id === id && one.kind === 'unit')) {
        router.go({ ...router.route, viewId: id, cardId: undefined });
      }
    },
  };

  async function readTree(id: string, targetKind: 'wiki' | 'course'): Promise<void> {
    treeReadings += 1;
    const reading = treeReadings;
    status = 'loading';
    try {
      const read: SharedNode[] =
        targetKind === 'wiki'
          ? ((await backend.wiki?.pages(id)) ?? [])
          : ((await backend.course?.units(id)) ?? []);
      if (reading !== treeReadings) return;
      [items, status] = [read, 'ready'];
    } catch {
      if (reading === treeReadings) status = 'failed';
    }
  }

  async function readItem(id: string): Promise<void> {
    itemReadings += 1;
    const reading = itemReadings;
    [shown, itemFailed] = [undefined, false];
    try {
      if (kind === 'wiki') {
        if (!backend.wiki) return;
        const page = await backend.wiki.page(containerId, id);
        if (reading === itemReadings) shown = page;
      } else {
        if (!backend.course) return;
        const unit = await backend.course.unit(containerId, id);
        if (reading === itemReadings) shown = unit.kind === 'chapter' ? null : unit;
      }
    } catch (error) {
      if (reading !== itemReadings) return;
      if (isBackendError(error) && error.status === 404) shown = null;
      else itemFailed = true;
    }
  }

  // Re-read the tree whenever containerId or kind changes.
  $effect(() => {
    void readTree(containerId, kind);
  });

  // Without an item in the address (or if a course link names a chapter): go to the first leaf.
  $effect(() => {
    if (status !== 'ready') return;
    if (!itemId) {
      const first = kind === 'wiki' ? firstPage(items) : firstUnit(items);
      if (first) router.go({ ...router.route, viewId: first.id }, { replace: true });
      return;
    }
    if (kind === 'course') {
      const node = items.find((item) => item.id === itemId);
      if (node?.kind === 'chapter') {
        const child = firstUnit(items.filter((item) => item.parentId === node.id));
        const fallback = child ?? firstUnit(items);
        if (fallback) router.go({ ...router.route, viewId: fallback.id }, { replace: true });
      }
    }
  });

  $effect(() => {
    const id = itemId;
    if (!id) return;
    if (kind === 'course') {
      if (status !== 'ready' || items.find((item) => item.id === id)?.kind === 'chapter') return;
    }
    void readItem(id);
  });

  $effect(() => {
    backend.presence.setPresence({ workspaceId: '', containerId, cardId: itemId ?? '' });
  });
  $effect(() => () => {
    backend.presence.setPresence(null);
  });

  const navLabel = $derived(kind === 'wiki' ? m.wiki_pages() : m.course_units());
  const untitledItem = $derived(
    kind === 'wiki' ? m.wiki_page_untitled() : m.course_unit_untitled(),
  );
  const emptyListText = $derived(kind === 'wiki' ? m.wiki_no_pages() : m.course_no_units());
  const emptyBodyText = $derived(
    kind === 'wiki' ? m.wiki_page_body_empty() : m.course_unit_body_empty(),
  );
  nameWindow(() => [shown ? shown.title || untitledItem : undefined, containerTitle]);
  const rootTestId = $derived(kind === 'wiki' ? 'shared-wiki' : 'shared-course');
  const treeTestId = $derived(kind === 'wiki' ? 'shared-page-tree' : 'shared-unit-tree');
  const articleTestId = $derived(kind === 'wiki' ? 'shared-page' : 'shared-unit');
  const titleTestId = $derived(kind === 'wiki' ? 'page-title' : 'unit-title');
  const bodyTestId = $derived(kind === 'wiki' ? 'page-body' : 'unit-body');
</script>

{#snippet problem(retry: () => void)}
  <div class="grid h-full place-items-center p-6 text-center">
    <div>
      <p class="text-muted-foreground" role="alert">{m.app_problem_load()}</p>
      <Button class="mt-4 roomy:min-h-touch" onclick={retry}>{m.app_retry()}</Button>
    </div>
  </div>
{/snippet}

{#snippet tree()}
  <nav
    id="{uid}-tree"
    aria-label={navLabel}
    class={[narrow && 'max-h-[50dvh] overflow-y-auto overscroll-contain']}
    data-testid={treeTestId}
  >
    <ul class="grid gap-0.5 p-2">
      {#each rows as row (row.page.id)}
        <li style:padding-left="{row.depth * 0.75}rem">
          {#if kind === 'course' && row.page.kind === 'chapter'}
            <span
              class="flex min-h-7 items-center truncate px-2 pt-2 text-xs font-semibold tracking-wide text-muted-foreground uppercase"
              data-kind="chapter"
            >
              {row.page.title || m.course_chapter_untitled()}
            </span>
          {:else}
            <a
              href={hrefOf(row.page.id)}
              aria-current={row.page.id === itemId ? 'page' : undefined}
              class="flex min-h-8 items-center gap-1.5 truncate rounded-md px-2 hover:bg-muted aria-[current=page]:bg-muted aria-[current=page]:font-medium roomy:min-h-touch"
              onclick={(event) => {
                if (event.metaKey || event.ctrlKey || event.shiftKey || event.button !== 0) return;
                event.preventDefault();
                treeOpen = false;
                router.go({ ...router.route, viewId: row.page.id, cardId: undefined });
              }}
            >
              {#if kind === 'wiki' && row.page.icon}
                <span aria-hidden="true">{row.page.icon}</span>
              {/if}
              <span class="truncate">{row.page.title || untitledItem}</span>
            </a>
          {/if}
        </li>
      {/each}
    </ul>
  </nav>
{/snippet}

{#if status === 'loading'}
  <div class="grid h-full place-items-center p-6">
    <p class="text-muted-foreground" role="status">{m.app_loading()}</p>
  </div>
{:else if status === 'failed'}
  {@render problem(() => void readTree(containerId, kind))}
{:else}
  <div class={['flex min-h-0 flex-1', narrow && 'flex-col']} data-testid={rootTestId}>
    {#if narrow}
      <div class="shrink-0 border-b">
        <div class="flex items-center">
          <button
            type="button"
            class="flex min-h-touch min-w-0 flex-1 items-center gap-2 px-4 text-sm text-muted-foreground"
            aria-expanded={treeOpen}
            aria-controls="{uid}-tree"
            onclick={() => {
              treeOpen = !treeOpen;
            }}
          >
            <ListTreeIcon class="size-4" aria-hidden="true" />
            {navLabel}
          </button>
          {#if outline.mode === 'button'}
            <div class="shrink-0 pr-2"><OutlineButton {outline} id="{uid}-outline" /></div>
          {/if}
        </div>
        {#if treeOpen}{@render tree()}{/if}
      </div>
    {:else}
      <div class="w-64 shrink-0 overflow-y-auto border-r">{@render tree()}</div>
    {/if}
    <article
      class="min-w-0 flex-1 overflow-y-auto px-4"
      data-testid={articleTestId}
      {@attach outline.follow}
    >
      {#if !narrow && outline.mode === 'button'}
        <div
          class="sticky top-0 z-10 -mx-4 flex justify-end bg-background/95 px-2 py-1"
          {@attach outline.bar}
        >
          <OutlineButton {outline} id="{uid}-outline" />
        </div>
      {/if}
      {#if !hasContentItems}
        <p class="py-6 text-muted-foreground">{emptyListText}</p>
      {:else if shown === null}
        <NotFound {router} />
      {:else if itemFailed && itemId}
        {@const id = itemId}
        {@render problem(() => void readItem(id))}
      {:else if shown}
        <OutlineColumns {outline} id="{uid}-outline">
          <div class="mx-auto max-w-3xl py-6">
            <h2 class="mb-4 text-2xl font-semibold break-words" data-testid={titleTestId}>
              {#if kind === 'wiki' && shown.icon}<span aria-hidden="true">
                  {shown.icon}
                </span>{/if}{shown.title || untitledItem}
            </h2>
            <div data-testid={bodyTestId} {@attach outline.source}>
              <BodyView doc={shown.body} {references}>
                {#snippet empty()}
                  <p class="text-muted-foreground">{emptyBodyText}</p>
                {/snippet}
              </BodyView>
            </div>
          </div>
        </OutlineColumns>
      {:else}
        <p class="py-6 text-muted-foreground" role="status">{m.app_loading()}</p>
      {/if}
    </article>
  </div>
{/if}
