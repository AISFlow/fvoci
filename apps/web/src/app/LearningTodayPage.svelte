<!--
  App-level wrapper for `/w/<workspace>/learning/today`: connects
  `LearningTodayView` to `LearningTodayStore`, device time-zone sync, and
  the app's `Router`.
-->
<script lang="ts">
  import { untrack } from 'svelte';

  import LearningTodayView from '$lib/features/course/LearningTodayView.svelte';
  import { LearningTodayStore, type LearningTodayItem, type Workspace } from '$lib/state/index.ts';

  import NotFound from './NotFound.svelte';
  import type { Router } from './router.svelte.ts';

  interface Props {
    workspace: Workspace;
    router: Router;
  }

  let { workspace, router }: Props = $props();

  const workspaceId = $derived(workspace.info?.id);
  const store = $derived(workspaceId ? new LearningTodayStore(workspace) : undefined);
  const serverTimeZone = $derived(
    workspace.settings.timeZone === 'auto' ? 'UTC' : workspace.settings.timeZone,
  );

  // Close a store only when replaced (workspace change) or unmounted, never on time-zone reload.
  $effect(() => {
    const current = store;
    return () => {
      current?.close();
    };
  });

  $effect(() => {
    const id = workspaceId;
    const current = store;
    // Read anew in another time zone, whose today may be another day.
    const _zone = workspace.settings.timeZone;
    if (!id || !current) return;
    untrack(() => {
      void current.load(id);
    });
  });

  function unitHref(item: LearningTodayItem): string {
    return router.href({ workspaceId, courseId: item.courseId, unitId: item.unitId });
  }

  function openUnit(item: LearningTodayItem): void {
    if (!workspaceId) return;
    router.go({ workspaceId, courseId: item.courseId, unitId: item.unitId });
  }
</script>

{#if workspaceId && store && workspace.backend.course}
  <LearningTodayView
    {store}
    {workspaceId}
    timeZone={serverTimeZone}
    {unitHref}
    onOpenUnit={openUnit}
    onRefresh={() => void store.load(workspaceId)}
  />
{:else}
  <NotFound {router} />
{/if}
