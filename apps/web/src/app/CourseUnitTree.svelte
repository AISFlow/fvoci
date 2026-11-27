<!--
  Course outline tree in the left column of `CoursePage.svelte`: wraps
  `PageTree` with unit completion icons, schedule badges, and chapter/unit
  creation header buttons.
-->
<script lang="ts">
  import CircleIcon from '@lucide/svelte/icons/circle';
  import CircleCheckIcon from '@lucide/svelte/icons/circle-check';

  import UnitTreeScheduleBadge from '$lib/features/course/UnitTreeScheduleBadge.svelte';
  import PageTree from '$lib/features/wiki/PageTree.svelte';
  import { COURSE_TREE_LABELS } from '$lib/features/wiki/tree-actions.ts';
  import type { TreeOpen } from '$lib/features/wiki/tree-open.svelte.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { PagePlace, TreeRules } from '$lib/logic/page-moves.ts';
  import type { CourseStore, CourseUnit, PageDraft } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  interface Props {
    course: CourseStore;
    treeOpen: TreeOpen;
    rules: TreeRules<CourseUnit>;
    creating: boolean;
    pageHref: (id: string) => string;
    closeDrawer: (restoreFocus?: boolean) => void;
    onOpenUnit: (id: string) => void;
    onCreateUnit: (draft: PageDraft, onCreated?: () => void) => void;
    onNewChapter: () => void;
    onRenameChapter: (chapter: CourseUnit) => void;
    onDeleteItem: (item: CourseUnit) => void;
    onMoveUnit: (id: string, to: PagePlace) => Promise<boolean>;
    onMoveTo: (chosen: CourseUnit) => void;
  }

  let {
    course,
    treeOpen,
    rules,
    creating,
    pageHref,
    closeDrawer,
    onOpenUnit,
    onCreateUnit,
    onNewChapter,
    onRenameChapter,
    onDeleteItem,
    onMoveUnit,
    onMoveTo,
  }: Props = $props();
</script>

<PageTree
  wiki={course}
  currentId={course.openUnit?.unitId}
  open={treeOpen}
  {pageHref}
  onOpenPage={(id: string) => {
    closeDrawer();
    onOpenUnit(id);
  }}
  onCreate={(draft: PageDraft) => {
    onCreateUnit(draft, () => {
      closeDrawer(false);
    });
  }}
  onRename={onRenameChapter}
  onDelete={onDeleteItem}
  onMove={onMoveUnit}
  {onMoveTo}
  {rules}
  labels={COURSE_TREE_LABELS}
  testId="unit-tree"
  busy={creating}
>
  {#snippet itemLeading(item: CourseUnit)}
    {#if course.canTrackProgress && item.kind === 'unit'}
      {@const done = course.progress.isCompleted(item.id)}
      <span
        class={['shrink-0', done ? 'text-primary' : 'text-muted-foreground/40']}
        data-testid="unit-tree-progress"
        data-completed={done ? 'true' : 'false'}
        aria-hidden="true"
      >
        {#if done}<CircleCheckIcon class="size-3.5" />{:else}<CircleIcon class="size-3.5" />{/if}
      </span>
      <span class="sr-only">{done ? m.course_unit_completed() : m.course_unit_incomplete()}</span>
    {/if}
  {/snippet}
  {#snippet itemTrailing(item: CourseUnit)}
    <UnitTreeScheduleBadge {course} unit={item} />
  {/snippet}
  {#snippet headerActions()}
    <div class="flex shrink-0 items-center gap-1">
      <Button
        variant="ghost"
        size="sm"
        class="h-7 px-2 text-xs text-muted-foreground roomy:min-h-touch"
        data-testid="new-chapter"
        disabled={creating}
        onclick={onNewChapter}
      >
        {m.course_new_chapter()}
      </Button>
      <Button
        variant="ghost"
        size="sm"
        class="h-7 px-2 text-xs text-muted-foreground roomy:min-h-touch"
        data-testid="new-unit"
        disabled={creating}
        onclick={() => {
          onCreateUnit({}, () => {
            closeDrawer(false);
          });
        }}
      >
        {m.course_new_unit()}
      </Button>
    </div>
  {/snippet}
</PageTree>
