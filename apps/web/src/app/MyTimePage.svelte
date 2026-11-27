<!--
  App-level wrapper for `/me/time`: connects `MyTimeScreen` to `TimeTracker`
  and the app's `Router`.
-->
<script lang="ts">
  import MyTimeScreen from '$lib/features/time/MyTimeScreen.svelte';
  import { timeTracker, type TimeEntry, type Workspace } from '$lib/state/index.ts';

  import NotFound from './NotFound.svelte';
  import type { Router } from './router.svelte.ts';
  import { openTimeEntryTarget, timeEntryTargetHref } from './time-places.ts';

  interface Props {
    workspace: Workspace;
    router: Router;
  }

  let { workspace, router }: Props = $props();

  const tracker = $derived(workspace.me?.id ? timeTracker(workspace) : undefined);
</script>

{#if tracker}
  <MyTimeScreen
    {tracker}
    locale={workspace.locale}
    regional={workspace.regional}
    targetHref={(entry: TimeEntry) => timeEntryTargetHref(router, entry)}
    onOpenTarget={(entry: TimeEntry) => {
      openTimeEntryTarget(router, entry);
    }}
  />
{:else}
  <NotFound {router} />
{/if}
