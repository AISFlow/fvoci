<!--
  A view of a board shown in a body, as a card's body shows it: the example
  board's one view, or a view that is not there.
-->
<script lang="ts">
  import { untrack } from 'svelte';

  import EmbeddedView from '../src/app/embedded/EmbeddedView.svelte';
  import { Router } from '../src/app/router.svelte.ts';
  import '$lib/editor/board-view.css';
  import { exampleWorkspace, loadWorkspace, type WorkspaceExample } from './workspace.ts';

  let {
    kind = 'table',
    count = 8,
    readOnly = false,
    narrow = false,
    missing = false,
  }: WorkspaceExample & { narrow?: boolean; missing?: boolean } = $props();

  const example = $derived(exampleWorkspace({ kind, count, readOnly }));
  const router = new Router('');
  let ready = $state(false);
  /** The card asked to open over the page: the app opens it in a dialog. */
  let opened = $state('');
</script>

<div
  class="doc-body grid gap-3 p-4 text-base"
  style:max-width={narrow ? '390px' : undefined}
  data-testid="board-view-example"
  {@attach () => {
    const current = example;
    untrack(() => {
      ready = false;
      void loadWorkspace(current).then(() => {
        ready = true;
      });
    });
    return () => {
      current.workspace.close();
    };
  }}
>
  <p>A line of the body before the view · 보드 뷰 앞의 문단</p>
  {#if ready}
    <div class="doc-board-view" data-board-view="">
      <EmbeddedView
        workspace={example.workspace}
        {router}
        viewId={missing ? 'example-view-gone' : 'example-view'}
        onOpenCard={(_boardId: string, _view: unknown, cardId: string) => {
          opened = cardId;
        }}
      />
    </div>
  {/if}
  <p>A line of the body after it · 보드 뷰 뒤의 문단</p>
  <p data-testid="opened-card">{opened}</p>
</div>
