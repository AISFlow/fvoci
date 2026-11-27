<!--
  Where the page is, at the head of the top bar: the reader's own space with a
  lock (the lock alone in a narrow pane), a shared space by its organization's
  name (`space-names.ts`, from where two kanban columns fit), and "Guest" for a
  reader who is only a guest there.
-->
<script lang="ts">
  import LockIcon from '@lucide/svelte/icons/lock';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import { pane } from '$lib/layout/room.svelte.ts';
  import { isGuest, isOwnSpace, organizationNames, type Workspace } from '$lib/state/index.ts';

  import { spaceName } from './space-names.ts';

  let { workspace }: { workspace: Workspace } = $props();

  const orgNames = $derived(organizationNames(workspace));
  $effect(() => {
    orgNames.follow(workspace.workspaces);
  });

  const shown = $derived(workspace.info);
  const name = $derived(
    spaceName(shown, workspace.workspaces, (orgId: string | undefined) => orgNames.of(orgId)),
  );
  const guest = $derived(isGuest(workspace));
</script>

{#if shown && (isOwnSpace(shown) || guest || pane.columns)}
  <span class="flex min-w-0 shrink-[2] items-center gap-1 pl-1" data-testid="space-badge">
    {#if isOwnSpace(shown)}
      <span class="flex shrink-0 items-center gap-1 text-sm text-muted-foreground" data-own-space>
        <LockIcon class="size-4" aria-hidden="true" />
        <span class="sr-only @columns/pane:not-sr-only">{name}</span>
      </span>
    {:else}
      <span class="min-w-0 truncate text-sm text-muted-foreground">
        {name}
      </span>
    {/if}
    {#if guest}
      <span
        class="shrink-0 rounded-md border px-1.5 py-0.5 text-xs text-muted-foreground"
        data-testid="guest-badge">{m.people_role_guest()}</span
      >
    {/if}
    <span aria-hidden="true" class="hidden text-muted-foreground/60 @columns/pane:inline">/</span>
  </span>
{/if}
