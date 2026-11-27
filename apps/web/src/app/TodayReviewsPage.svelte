<!--
  App-level wrapper for `/w/<workspace>/learning/reviews/today`: connects
  `TodayReviewsView` to `TodayReviewsStore` and the app's `Router`.
-->
<script lang="ts">
  import { untrack } from 'svelte';

  import TodayReviewsView from '$lib/features/course/TodayReviewsView.svelte';
  import { todayReviews, type TodayReviewItem, type Workspace } from '$lib/state/index.ts';

  import NotFound from './NotFound.svelte';
  import type { Router } from './router.svelte.ts';

  interface Props {
    workspace: Workspace;
    router: Router;
  }

  let { workspace, router }: Props = $props();

  const workspaceId = $derived(workspace.info?.id);
  const store = $derived(
    workspace.backend.course && workspaceId && workspace.me?.id
      ? todayReviews(workspace)
      : undefined,
  );

  $effect(() => {
    const id = workspaceId;
    const current = store;
    // Read anew in another time zone, whose today may be another day.
    const _zone = workspace.settings.timeZone;
    const _version = current?.version;
    if (!id || !current) return;
    untrack(() => {
      void current.load(id);
    });
  });

  function unitRoute(item: TodayReviewItem) {
    return { workspaceId, courseId: item.containerId, unitId: item.unitId };
  }
</script>

{#if store && workspaceId}
  <TodayReviewsView
    {store}
    {workspaceId}
    autoLoad={false}
    unitHref={(item: TodayReviewItem) => router.href(unitRoute(item))}
    onOpenUnit={(item: TodayReviewItem) => {
      router.go(unitRoute(item));
    }}
  />
{:else}
  <NotFound {router} />
{/if}
