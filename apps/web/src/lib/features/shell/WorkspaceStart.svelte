<!--
  The pane of a workspace with nothing in it yet: says so and leads to making
  a board or a wiki (`onAddBoard`, the shell's picker). The reader's own space
  says what it is first: theirs alone, shown to others by a public link only.
-->
<script lang="ts">
  import PlusIcon from '@lucide/svelte/icons/plus';
  import UserRoundIcon from '@lucide/svelte/icons/user-round';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import { Button } from '$lib/ui/button/index.js';

  interface Props {
    /** The workspace is the reader's own space. */
    own: boolean;
    /** Opens the picker of what to make; left out where the reader makes nothing. */
    onAddBoard?: (() => void) | undefined;
  }

  let { own, onAddBoard }: Props = $props();
</script>

<div class="grid h-full place-items-center p-6 text-center" data-testid="workspace-start">
  <div class="flex max-w-sm flex-col items-center gap-3">
    {#if own}
      <UserRoundIcon class="size-8 text-muted-foreground" aria-hidden="true" />
      <h1 class="text-lg font-semibold">{m.own_space_name()}</h1>
      <p class="text-muted-foreground">{m.own_space_start()}</p>
    {:else}
      <p class="text-muted-foreground">{m.app_no_boards()}</p>
    {/if}
    {#if onAddBoard}
      <Button class="mt-1 roomy:min-h-touch" data-testid="workspace-start-add" onclick={onAddBoard}>
        <PlusIcon aria-hidden="true" />
        {m.shell_add_board()}
      </Button>
    {/if}
  </div>
</div>
