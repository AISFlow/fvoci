<!--
  The account's hub of today (`/today`), across the spaces open: a line for
  each diary of the reader's own space, the cards due (by board, each made done
  at a press), the units due (each put off at a press), the reviews and the
  meetings. Only what has something in it is shown.
-->
<script lang="ts">
  import { untrack } from 'svelte';

  import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';

  import { spaceName } from '$lib/features/shell/space-names.ts';
  import TodayCards from '$lib/features/today/TodayCards.svelte';
  import TodayDiaries from '$lib/features/today/TodayDiaries.svelte';
  import TodayUnits from '$lib/features/today/TodayUnits.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { nameWindow } from '$lib/layout/window-title.svelte.ts';
  import { displayDateTime } from '$lib/logic/format.ts';
  import {
    organizationNames,
    TodayHubStore,
    type TodayCardItem,
    type TodayDiary,
    type TodayUnitEntry,
    type TodayWorkspaceMeta,
    type Workspace,
  } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import { goToPlace } from './places.ts';
  import type { Router } from './router.svelte.ts';
  import { askToWriteToday } from './write-today.ts';

  interface Props {
    workspace: Workspace;
    router: Router;
  }

  let { workspace, router }: Props = $props();

  const store = $derived(new TodayHubStore(workspace));
  nameWindow(() => [m.today_hub_title()]);

  $effect(() => {
    const current = store;
    return () => {
      current.close();
    };
  });

  $effect(() => {
    const current = store;
    // Read anew in another time zone, whose today may be another day.
    const _zone = workspace.settings.timeZone;
    untrack(() => {
      void current.load();
    });
  });

  const data = $derived(store.data);
  const isEmpty = $derived(
    data?.cards.length === 0 &&
      data.units.length === 0 &&
      data.reviews.length === 0 &&
      data.meetings.length === 0 &&
      store.diaries.every((diary) => diary.writtenToday),
  );

  const orgNames = $derived(organizationNames(workspace));
  /** The space of an item, named as the switcher names it; one not listed by the name it came with. */
  function spaceOf(item: { workspace: TodayWorkspaceMeta }): string {
    const meta = item.workspace;
    if (meta.isPersonal) return m.own_space_name();
    const info = workspace.workspaces.find((one) => one.id === meta.workspaceId);
    if (!info) return meta.workspaceName || m.shell_workspace_unnamed();
    const orgName = (orgId: string | undefined) =>
      orgNames.of(orgId) || (orgId === meta.orgId ? meta.orgName : '');
    return spaceName(info, workspace.workspaces, orgName, m.shell_workspace_unnamed);
  }

  function openCard(item: TodayCardItem): void {
    goToPlace(router, {
      kind: 'card',
      id: item.cardId,
      containerId: item.boardId,
      workspaceId: item.workspace.workspaceId,
    });
  }

  function openDiary(diary: TodayDiary, write: boolean): void {
    if (write) askToWriteToday(diary.diaryId);
    router.go({ workspaceId: diary.workspaceId, diaryId: diary.diaryId });
  }

  function openUnit(workspaceId: string, containerId: string, unitId: string): void {
    goToPlace(router, { kind: 'unit', id: unitId, containerId, workspaceId });
  }
</script>

<div
  class="h-full overflow-y-auto overscroll-contain p-4 @columns/pane:p-6"
  data-testid="today-hub-page"
>
  <div class="mx-auto grid max-w-4xl gap-6">
    <div class="flex flex-wrap items-start justify-between gap-3">
      <div>
        <h2 class="text-xl font-semibold">{m.today_hub_title()}</h2>
        <p class="text-sm text-muted-foreground">{m.today_hub_subtitle()}</p>
      </div>
      <Button
        variant="outline"
        size="sm"
        class="roomy:min-h-touch"
        disabled={store.status === 'loading'}
        onclick={() => void store.load()}
        data-testid="today-hub-refresh"
      >
        <RefreshCwIcon class="size-4" aria-hidden="true" />
        {m.learning_today_refresh()}
      </Button>
    </div>

    {#if data && data.lockedWorkspaceCount > 0}
      <p
        class="rounded-md border bg-muted/60 px-3 py-2 text-sm text-muted-foreground"
        data-testid="today-hub-locked"
      >
        {m.today_hub_locked_notice({ count: data.lockedWorkspaceCount })}
      </p>
    {/if}

    {#if store.diaries.length > 0}
      <TodayDiaries
        diaries={store.diaries}
        onWrite={(diary: TodayDiary) => {
          openDiary(diary, true);
        }}
        onOpen={(diary: TodayDiary) => {
          openDiary(diary, false);
        }}
      />
    {/if}

    {#if store.status === 'loading' && !data}
      <p class="py-8 text-center text-muted-foreground" role="status">{m.app_loading()}</p>
    {:else if store.status === 'failed' && !data}
      <p class="py-8 text-center text-muted-foreground" role="alert">{m.app_problem_load()}</p>
    {:else if isEmpty}
      <p class="py-8 text-center text-muted-foreground" data-testid="today-hub-empty">
        {m.today_hub_empty()}
      </p>
    {:else if data}
      <TodayCards {store} {spaceOf} onOpen={openCard} />
      <TodayUnits
        {store}
        {spaceOf}
        onOpen={(unit: TodayUnitEntry) => {
          openUnit(unit.workspace.workspaceId, unit.courseId, unit.unitId);
        }}
      />

      {#if data.reviews.length > 0}
        <section class="grid gap-2" data-testid="today-hub-reviews">
          <h3 class="text-sm font-semibold text-muted-foreground">{m.today_hub_reviews()}</h3>
          <ul class="grid gap-1.5">
            {#each data.reviews as review (review.id)}
              <li>
                <button
                  type="button"
                  class="flex w-full items-center justify-between gap-3 rounded-md border px-3 py-2 text-left hover:bg-accent/50 roomy:min-h-touch"
                  onclick={() => {
                    openUnit(review.workspace.workspaceId, review.containerId, review.unitId);
                  }}
                >
                  <span class="min-w-0 flex-1 truncate font-medium">
                    {review.unitTitle || m.course_unit_untitled()}
                  </span>
                  <span class="shrink-0 text-xs text-muted-foreground">
                    {spaceOf(review)} · {review.courseTitle || m.course_untitled()} · {review.scheduledDate}
                  </span>
                </button>
              </li>
            {/each}
          </ul>
        </section>
      {/if}

      {#if data.meetings.length > 0}
        <section class="grid gap-2" data-testid="today-hub-meetings">
          <h3 class="text-sm font-semibold text-muted-foreground">{m.today_hub_meetings()}</h3>
          <ul class="grid gap-1.5">
            {#each data.meetings as meeting (meeting.meetingId)}
              <li>
                <button
                  type="button"
                  class="flex w-full items-center justify-between gap-3 rounded-md border px-3 py-2 text-left hover:bg-accent/50 roomy:min-h-touch"
                  onclick={() => {
                    goToPlace(router, {
                      kind: 'meeting',
                      id: meeting.meetingId,
                      containerId: meeting.containerId,
                      workspaceId: meeting.workspace.workspaceId,
                    });
                  }}
                >
                  <span class="min-w-0 flex-1 truncate font-medium">{meeting.title}</span>
                  <span class="shrink-0 text-xs text-muted-foreground">
                    {spaceOf(meeting)} · {displayDateTime(meeting.startsAt, {
                      locale: workspace.locale,
                      timeZone: workspace.regional.timeZone,
                      hourCycle: workspace.regional.hourCycle,
                    })}
                  </span>
                </button>
              </li>
            {/each}
          </ul>
        </section>
      {/if}
    {/if}
  </div>
</div>
