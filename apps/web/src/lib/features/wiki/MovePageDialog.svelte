<!--
  "Move…": places a page or unit below the chosen parent (last among its
  children) or at the top of the wiki/course. Nodes that cannot accept the
  moving node (itself, its descendants, or disallowed by `rules`) are disabled.
-->
<script lang="ts" generics="P extends TreeItem">
  import { untrack } from 'svelte';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import { allowsUnder, type PagePlace, type TreeRules } from '$lib/logic/page-moves.ts';
  import { pageRows, withDescendants } from '$lib/logic/page-tree.ts';
  import type { TreeItem, TreeSource } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';
  import * as Dialog from '$lib/ui/dialog/index.js';

  import { WIKI_MOVE_LABELS, type MoveDialogLabels } from './tree-actions.ts';

  interface Props {
    open: boolean;
    wiki: TreeSource<P>;
    /** The page or unit to move; the dialog shows nothing without one. */
    pageId: string | undefined;
    rules?: TreeRules<P> | undefined;
    labels?: MoveDialogLabels | undefined;
    onMove: (pageId: string, to: PagePlace) => void;
  }

  let {
    open = $bindable(),
    wiki,
    pageId,
    rules,
    labels = WIKI_MOVE_LABELS,
    onMove,
  }: Props = $props();

  const uid = $props.id();
  let parentId = $state('');

  const page = $derived(wiki.page(pageId));
  const rows = $derived(pageRows(wiki.tree, () => true));
  const moving = $derived(
    pageId === undefined ? new Set<string>() : withDescendants(wiki.pages, pageId),
  );
  const titleOf = (one: P): string => one.title || labels.untitled(one);

  function opened(): void {
    untrack(() => {
      parentId = page?.parentId ?? '';
    });
  }

  function submit(event: SubmitEvent): void {
    event.preventDefault();
    open = false;
    if (!page || page.parentId === parentId) return;
    onMove(page.id, { parentId });
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content
    class="max-h-[calc(100dvh-2rem-var(--safe-top)-var(--safe-bottom)-var(--keyboard))] overflow-y-auto"
    data-testid="move-page"
    {@attach opened}
  >
    {#if page}
      <form class="grid min-h-0 gap-4" onsubmit={submit}>
        <Dialog.Header>
          <Dialog.Title>{labels.title(titleOf(page))}</Dialog.Title>
          <Dialog.Description id="{uid}-about">{labels.description()}</Dialog.Description>
        </Dialog.Header>
        <div
          role="radiogroup"
          aria-labelledby="{uid}-about"
          class="max-h-[50dvh] overflow-y-auto overscroll-contain rounded-md border p-1"
        >
          <label
            class="flex min-h-8 cursor-pointer items-center gap-2 rounded-sm px-2 hover:bg-muted roomy:min-h-touch"
          >
            <input type="radio" name="{uid}-parent" value="" bind:group={parentId} />
            <span class="font-medium">{labels.top()}</span>
          </label>
          {#each rows as row (row.page.id)}
            {@const allowed =
              !moving.has(row.page.id) && allowsUnder(wiki.pages, page, row.page, rules)}
            <label
              class={[
                'flex min-h-8 items-center gap-2 rounded-sm px-2 roomy:min-h-touch',
                allowed ? 'cursor-pointer hover:bg-muted' : 'text-muted-foreground',
              ]}
              style:padding-left="{0.5 + (row.depth + 1) * 1}rem"
              data-target={row.page.id}
            >
              <input
                type="radio"
                name="{uid}-parent"
                value={row.page.id}
                disabled={!allowed}
                aria-label={m.wiki_move_target({ title: titleOf(row.page), level: row.depth + 1 })}
                bind:group={parentId}
              />
              {#if row.page.icon}<span aria-hidden="true">{row.page.icon}</span>{/if}
              <span class="min-w-0 truncate">{titleOf(row.page)}</span>
            </label>
          {/each}
        </div>
        <Dialog.Footer>
          <Button
            type="button"
            variant="outline"
            onclick={() => {
              open = false;
            }}
          >
            {m.shared_cancel()}
          </Button>
          <Button type="submit" disabled={page.parentId === parentId}>
            {m.wiki_move_confirm()}
          </Button>
        </Dialog.Footer>
      </form>
    {/if}
  </Dialog.Content>
</Dialog.Root>
