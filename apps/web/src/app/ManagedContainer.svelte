<!--
  A board or a wiki for the invited only that the reader, an administrator of
  the workspace, manages without reading: in place of its content, its title,
  a line saying that joining shows the content and that every member reads
  that it was joined, the button that joins it (as a viewer), and the way to
  its members (the share dialog, without the public link, which is content).
  Anything else not there is `NotFound`.
-->
<script lang="ts">
  import { untrack } from 'svelte';

  import ShareDialog from '$lib/features/shell/ShareDialog.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { ContainerKind } from '$lib/logic/wire.ts';
  import { joinManaged, readManagedOnly, type Managed } from '$lib/state/managed-container.ts';
  import type { Workspace } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import NotFound from './NotFound.svelte';
  import type { Router } from './router.svelte.ts';

  interface Props {
    workspace: Workspace;
    store: Managed;
    /** Which of the two it is: a board's address names a board, a wiki's a wiki. */
    kind?: ContainerKind;
    /** Whether it is still the one shown: joined after it went, it is not read. */
    shown: () => boolean;
    router: Router;
    /** It is joined and shown: the page takes the focus, where the button was. */
    onJoined: () => void;
  }

  let { workspace, store, kind = 'board', shown, router, onJoined }: Props = $props();

  const wiki = $derived(kind === 'wiki');
  const course = $derived(kind === 'course');

  let managed = $state<'asking' | 'yes' | 'no'>('asking');
  let joining = $state(false);
  let membersOpen = $state(false);

  // Asked once for the board shown: is it there to be managed?
  $effect(() => {
    const read = store;
    managed = 'asking';
    // What the reading looks at besides the store is not what this waits for.
    const asked = kind;
    void untrack(() => readManagedOnly(read, asked)).then(
      (yes) => {
        if (read === store) managed = yes ? 'yes' : 'no';
      },
      () => {
        if (read === store) managed = 'no';
      },
    );
  });

  async function join(): Promise<void> {
    if (joining) return;
    joining = true;
    const joined = await joinManaged(store, shown);
    joining = false;
    if (joined) onJoined();
  }
</script>

{#if managed === 'asking'}
  <div class="grid h-full place-items-center p-6">
    <p class="text-muted-foreground" role="status">{m.app_loading()}</p>
  </div>
{:else if managed === 'no' || !store.container}
  <NotFound {router} />
{:else}
  <div
    class="grid h-full place-items-center p-6 text-center"
    data-testid={wiki ? 'wiki-managed' : course ? 'course-managed' : 'board-managed'}
  >
    <div class="flex max-w-md flex-col items-center gap-3">
      <h2 class="text-lg font-medium">
        {store.container.icon ? `${store.container.icon} ` : ''}{store.container.title ||
          (wiki ? m.wiki_untitled() : course ? m.course_untitled() : m['Sidebar.untitled-board']())}
      </h2>
      <p class="text-muted-foreground">
        {wiki
          ? m.app_wiki_managed_hint()
          : course
            ? m.app_course_managed_hint()
            : m.app_board_managed_hint()}
      </p>
      <div class="flex flex-wrap justify-center gap-2">
        <Button aria-disabled={joining} aria-busy={joining} onclick={() => void join()}
          >{m.app_board_managed_join()}</Button
        >
        <Button
          variant="outline"
          aria-haspopup="dialog"
          onclick={() => {
            membersOpen = true;
          }}>{m.app_board_managed_members()}</Button
        >
      </div>
    </div>
  </div>
  <ShareDialog
    bind:open={membersOpen}
    {workspace}
    shareable={store}
    linkOf={(readToken: string) =>
      new URL(router.href({ boardId: store.containerId, readToken }), location.href).href}
    membersOnly
  />
{/if}
