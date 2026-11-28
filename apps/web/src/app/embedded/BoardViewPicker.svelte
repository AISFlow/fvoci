<!--
  The window that asks which view to show in a body (`/보드 뷰`): the boards
  of the workspace first, then the views of the one chosen, each with its
  kind. Choosing a view is done with it; closing it chooses none.
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import ArrowLeftIcon from '@lucide/svelte/icons/arrow-left';
  import XIcon from '@lucide/svelte/icons/x';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { closeOnBack } from '$lib/layout/back.svelte.ts';
  import { page } from '$lib/layout/room.svelte.ts';
  import type { BoardView, Container } from '$lib/logic/wire.ts';
  import { boardsToShow, viewsOfBoard, type Workspace } from '$lib/state/index.ts';
  import * as Dialog from '$lib/ui/dialog/index.js';

  import { kindName } from './embedded-view.ts';

  interface Props {
    open: boolean;
    workspace: Workspace;
    onPick: (viewId: string) => void;
  }

  let { open = $bindable(), workspace, onPick }: Props = $props();

  const compact = $derived(page.stage === 'compact');
  const boards = $derived(boardsToShow(workspace));

  /** The board chosen, and its views once read; `failed` when they could not be. */
  let board = $state.raw<Container | undefined>(undefined);
  let views = $state.raw<BoardView[] | 'failed' | undefined>(undefined);
  /** Readings begun: only the latest is shown. */
  let readings = 0;

  closeOnBack(
    () => open,
    () => {
      open = false;
    },
  );

  /** Each time the window opens: the boards first. */
  function opened(): void {
    untrack(() => {
      back();
    });
  }

  function chooseBoard(chosen: Container): void {
    board = chosen;
    views = undefined;
    readings += 1;
    const reading = readings;
    viewsOfBoard(workspace, chosen.id).then(
      (read) => {
        if (reading === readings) views = read;
      },
      () => {
        if (reading === readings) views = 'failed';
      },
    );
  }

  function back(): void {
    board = undefined;
    views = undefined;
    readings += 1;
  }

  const boardName = (container: Container): string =>
    container.title || m['Sidebar.untitled-board']();
</script>

<Dialog.Root bind:open>
  <Dialog.Content
    {@attach opened}
    data-testid="board-view-picker"
    showCloseButton={false}
    class={[
      'flex flex-col gap-0 p-0',
      compact
        ? 'top-0 left-0 h-dvh max-w-none translate-x-0 translate-y-0 rounded-none pt-(--safe-top) pr-(--safe-right) pb-(--safe-bottom) pl-(--safe-left) ring-0 sm:max-w-none'
        : 'max-h-[calc(100dvh-2rem-var(--safe-top)-var(--safe-bottom))] w-[min(28rem,calc(100%-2rem))] max-w-none sm:max-w-none',
    ]}
  >
    <Dialog.Header class="shrink-0 flex-row items-center gap-2 border-b py-2 pr-2 pl-2">
      {#if board}
        <IconButton label={m.board_view_pick_back()} onclick={back}>
          <ArrowLeftIcon />
        </IconButton>
      {/if}
      <div class={['min-w-0 flex-1 py-1', !board && 'pl-2']}>
        <Dialog.Title class="truncate text-base font-semibold">
          {board ? boardName(board) : m.board_view_pick_title()}
        </Dialog.Title>
        <Dialog.Description class="text-muted-foreground">
          {board ? m.board_view_pick_view({ board: boardName(board) }) : m.board_view_pick_board()}
        </Dialog.Description>
      </div>
      <Dialog.Close>
        {#snippet child({ props }: { props: Record<string, unknown> })}
          <IconButton {...props} label={m.ui_close()}>
            <XIcon />
          </IconButton>
        {/snippet}
      </Dialog.Close>
    </Dialog.Header>
    <div class="min-h-0 flex-1 overflow-y-auto overscroll-contain p-2">
      {#if !board}
        {#if boards.length === 0}
          <p class="p-2 text-muted-foreground">{m.board_view_pick_no_boards()}</p>
        {:else}
          <ul class="grid gap-0.5" data-testid="board-view-pick-boards">
            {#each boards as each (each.id)}
              <li>
                <button
                  type="button"
                  class="flex min-h-10 w-full items-center gap-2 rounded-md px-2.5 py-1.5 text-start hover:bg-accent focus-visible:bg-accent roomy:min-h-touch"
                  onclick={() => {
                    chooseBoard(each);
                  }}
                >
                  {#if each.icon}<span aria-hidden="true">{each.icon}</span>{/if}
                  <span class="truncate">{boardName(each)}</span>
                </button>
              </li>
            {/each}
          </ul>
        {/if}
      {:else if views === undefined}
        <p class="p-2 text-muted-foreground" role="status">{m.board_view_loading()}</p>
      {:else if views === 'failed'}
        <p class="p-2 text-muted-foreground" role="alert">{m.board_view_failed()}</p>
      {:else if views.length === 0}
        <p class="p-2 text-muted-foreground">{m.board_view_pick_no_views()}</p>
      {:else}
        <ul class="grid gap-0.5" data-testid="board-view-pick-views">
          {#each views as view (view.id)}
            <li>
              <button
                type="button"
                class="flex min-h-10 w-full items-center gap-2 rounded-md px-2.5 py-1.5 text-start hover:bg-accent focus-visible:bg-accent roomy:min-h-touch"
                onclick={() => {
                  // Told first: closing without a view chosen chooses none.
                  onPick(view.id);
                  open = false;
                }}
              >
                <span class="min-w-0 flex-1 truncate">
                  {view.title || m['Sidebar.untitled-view']()}
                </span>
                <span class="embedded-view-kind">{kindName(view.fields.viewType)}</span>
              </button>
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  </Dialog.Content>
</Dialog.Root>
