<script lang="ts">
  import { untrack } from 'svelte';
  import Calendar from '$lib/features/views/calendar/Calendar.svelte';
  import Gantt from '$lib/features/views/gantt/Gantt.svelte';
  import Table from '$lib/features/views/table/Table.svelte';
  import Kanban from '$lib/features/views/kanban/Kanban.svelte';
  import Gallery from '$lib/features/views/gallery/Gallery.svelte';
  import ViewHeader from '$lib/features/view-header/ViewHeader.svelte';
  import { scales } from '$lib/features/views/calendar/period.svelte.ts';
  import { pane } from '$lib/layout/room.svelte.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { exampleWorkspace, loadWorkspace, type WorkspaceExample } from './workspace.ts';

  let {
    kind = 'calendar',
    count = 8,
    readOnly = false,
    cycle = false,
    late = false,
    variedRelations = false,
    narrow = false,
    week = false,
    headerOnly = false,
  }: WorkspaceExample & { narrow?: boolean; week?: boolean; headerOnly?: boolean } = $props();
  const example = $derived(
    exampleWorkspace({ kind, count, readOnly, cycle, late, variedRelations }),
  );
  const board = $derived(example.workspace.board);
  const view = $derived(board?.view('example-view'));
  let search = $state('');
  let opened = $state('');
  const cards = $derived(board && view ? board.cardsOf(view, search) : []);
  const Component = $derived(
    { calendar: Calendar, gantt: Gantt, table: Table, board: Kanban, gallery: Gallery }[kind],
  );
</script>

<div
  class="@container/pane flex h-[40rem] min-h-0 flex-col border"
  style:max-width={narrow ? '390px' : undefined}
  data-testid="view-example"
  {@attach pane.attach}
  {@attach () => {
    const current = example;
    const scale = week ? 'week' : 'month';
    untrack(() => {
      scales.set('example-view', scale);
      void loadWorkspace(current);
    });
    return () => {
      current.workspace.close();
    };
  }}
>
  {#if board?.status === 'ready' && view}
    {#if headerOnly}
      <ViewHeader
        {board}
        {view}
        bind:search
        viewHref={(id) => `#${id}`}
        onOpenView={(id: string) => {
          opened = id;
        }}
        onAddCard={() => {
          void board.createCard({}).then((card) => {
            opened = card?.id ?? '';
          });
        }}
      />
    {:else}
      <Component
        {board}
        {view}
        {cards}
        cardHref={(id) => `#${id}`}
        onOpenCard={(id: string) => {
          opened = id;
        }}
      />
    {/if}
  {:else}
    <p role="status">{m.app_loading()}</p>
  {/if}
</div>
<output data-testid="opened-card">{opened}</output>
