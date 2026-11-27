<!--
  A board, wiki, or course read by its public link (`/shared/<container>/<view>/<card>?r=<token>`).
  Reacts to `router.route` changes (`popstate`) so navigating Back/Forward between
  two shared links reloads cleanly (`W1.5`).
-->
<script lang="ts" module>
  import { LazyChunk } from '$lib/chunk-reload.svelte.ts';

  const sharedBoardChunk = new LazyChunk(() => import('./BoardPage.svelte'));
  const sharedTreeChunk = new LazyChunk(() => import('./SharedTreeReader.svelte'));
</script>

<script lang="ts">
  import { untrack } from 'svelte';

  import FileViewerHost from '$lib/components/file-viewer/FileViewerHost.svelte';
  import { setViewedFileSource } from '$lib/components/file-viewer/viewing.svelte.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { pane } from '$lib/layout/room.svelte.ts';
  import type { ContainerKind } from '$lib/logic/wire.ts';
  import { fileSourceOf } from '$lib/state/file-source.ts';
  import { isBackendError, Workspace, type Backend } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import { startShared } from './backend.ts';
  import LazyView from './LazyView.svelte';
  import { currentLocale } from './locale.ts';
  import NotFound from './NotFound.svelte';
  import Problems from './Problems.svelte';
  import type { Router } from './router.svelte.ts';

  let { router }: { router: Router } = $props();

  let workspace = $state.raw<Workspace | undefined>(undefined);
  let backend = $state.raw<Backend | undefined>(undefined);
  let kind = $state<ContainerKind | 'missing' | 'failed' | undefined>(undefined);
  let readKind: (() => Promise<void>) | undefined;
  let meta = $state.raw<{ title: string; icon: string } | undefined>(undefined);
  let place = '';
  let unstarted = $state(false);

  const sharedContainerId = $derived(
    router.route.kind === 'shared' ? router.route.boardId : undefined,
  );
  const sharedReadToken = $derived(
    router.route.kind === 'shared' ? router.route.readToken : undefined,
  );
  const container = $derived(kind === 'wiki' || kind === 'course' ? meta : workspace?.board?.board);
  const containerTitle = $derived(
    container?.title ||
      (kind === 'wiki'
        ? m.wiki_untitled()
        : kind === 'course'
          ? m.course_untitled()
          : m['Sidebar.untitled-board']()),
  );

  $effect(() => {
    const containerId = sharedContainerId;
    const readToken = sharedReadToken;
    untrack(() => {
      workspace = undefined;
      backend = undefined;
      kind = undefined;
      readKind = undefined;
      meta = undefined;
      unstarted = false;
    });
    if (!containerId || !readToken) return undefined;
    let made: Workspace | undefined;
    let left = false;
    const isLeft = (): boolean => left;
    void startShared(containerId, readToken).then(
      async (started) => {
        if (left) return;
        const read = started.backend;
        place = started.place;
        const shown = new Workspace({ backend: read, locale: currentLocale });
        made = shown;
        const files = fileSourceOf(
          read,
          () => containerId,
          (error) => {
            shown.report('load', error);
          },
        );
        setViewedFileSource(files);
        workspace = made;
        backend = read;
        await made.load(place);
        if (isLeft()) return;
        readKind = async () => {
          if (isLeft()) return;
          kind = undefined;
          try {
            const opened = await read.container(containerId);
            if (isLeft()) return;
            kind = opened.kind ?? 'board';
            if (opened.kind === 'wiki' || opened.kind === 'course') {
              meta = { title: opened.title, icon: opened.icon ?? '' };
            }
          } catch (error) {
            if (isLeft()) return;
            kind = isBackendError(error) && error.status === 404 ? 'missing' : 'failed';
          }
        };
        await readKind();
      },
      () => {
        if (!left) unstarted = true;
      },
    );
    return () => {
      left = true;
      made?.close();
    };
  });
</script>

<div
  class="flex h-[calc(100dvh-var(--safe-top))] min-h-0 flex-col overflow-hidden bg-background"
  data-testid="shared-board"
>
  <header class="flex h-12 shrink-0 items-center gap-3 border-b px-4">
    <p class="shrink-0 font-semibold tracking-tight" translate="no">{m.app_name()}</p>
    {#if container}
      <h1 class="min-w-0 truncate text-muted-foreground" data-testid="shared-title">
        {container.icon ? `${container.icon} ` : ''}{containerTitle}
      </h1>
    {/if}
  </header>
  <main class="@container/pane flex min-h-0 min-w-0 flex-1 flex-col" {@attach pane.attach}>
    {#if !sharedContainerId || !sharedReadToken}
      <NotFound {router} />
    {:else if unstarted}
      <div class="grid h-full place-items-center p-6 text-center">
        <p class="text-muted-foreground" role="alert">{m.app_problem_load()}</p>
      </div>
    {:else if !workspace || !kind || workspace.status === 'idle' || workspace.status === 'loading'}
      <div class="grid h-full place-items-center p-6">
        <p class="text-muted-foreground" role="status">{m.app_loading()}</p>
      </div>
    {:else if workspace.status !== 'ready'}
      <div class="grid h-full place-items-center p-6 text-center">
        <div>
          <p class="text-muted-foreground" role="alert">{m.app_problem_load()}</p>
          <Button class="mt-4 roomy:min-h-touch" onclick={() => void workspace?.load(place)}>
            {m.app_retry()}
          </Button>
        </div>
      </div>
    {:else if kind === 'missing'}
      <NotFound {router} />
    {:else if kind === 'failed'}
      <div class="grid h-full place-items-center p-6 text-center">
        <div>
          <p class="text-muted-foreground" role="alert">{m.app_problem_load()}</p>
          <Button class="mt-4 roomy:min-h-touch" onclick={() => void readKind?.()}>
            {m.app_retry()}
          </Button>
        </div>
      </div>
    {:else if (kind === 'wiki' || kind === 'course') && backend}
      {@const treeKind = kind}
      {@const treeBackend = backend}
      <LazyView chunk={sharedTreeChunk}>
        {#snippet children(SharedTreeReader)}
          <SharedTreeReader
            kind={treeKind}
            backend={treeBackend}
            containerId={sharedContainerId}
            containerTitle={container ? containerTitle : undefined}
            {router}
          />
        {/snippet}
      </LazyView>
    {:else if kind === 'diary'}
      <NotFound {router} />
    {:else}
      {@const shownWorkspace = workspace}
      <LazyView chunk={sharedBoardChunk}>
        {#snippet children(BoardPage)}
          <BoardPage workspace={shownWorkspace} {router} />
        {/snippet}
      </LazyView>
    {/if}
  </main>
</div>
<FileViewerHost regional={workspace?.regional} />
{#if workspace}
  <Problems {workspace} />
{/if}
