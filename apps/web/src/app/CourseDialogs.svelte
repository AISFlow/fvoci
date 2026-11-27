<!--
  Dialogs owned by an open course screen: moving a unit, creating/renaming a
  chapter, confirming unit/chapter or course deletion, unit trash, sharing, and
  course settings.
-->
<script lang="ts">
  import ConfirmDialog from '$lib/components/confirm/ConfirmDialog.svelte';
  import ChapterNameDialog from '$lib/features/course/ChapterNameDialog.svelte';
  import CourseSettingsDialog from '$lib/features/course/CourseSettingsDialog.svelte';
  import ShareDialog from '$lib/features/shell/ShareDialog.svelte';
  import TrashDialog from '$lib/features/shell/TrashDialog.svelte';
  import MovePageDialog from '$lib/features/wiki/MovePageDialog.svelte';
  import { COURSE_MOVE_LABELS } from '$lib/features/wiki/tree-actions.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { PagePlace, TreeRules } from '$lib/logic/page-moves.ts';
  import { withDescendants } from '$lib/logic/page-tree.ts';
  import type { CourseLooks, CourseStore, CourseUnit, Trash, Workspace } from '$lib/state/index.ts';

  import type { Router } from './router.svelte.ts';

  interface Props {
    workspace: Workspace;
    store: CourseStore;
    router: Router;
    rules: TreeRules<CourseUnit>;
    moving: string | undefined;
    movingOpen: boolean;
    onMoveUnit: (id: string, to: PagePlace) => void;
    chapterDialog: { mode: 'create' } | { mode: 'rename'; chapter: CourseUnit };
    chapterOpen: boolean;
    onSubmitChapter: (title: string) => void;
    deleting: CourseUnit | undefined;
    deleteOpen: boolean;
    trash: Trash | undefined;
    trashOpen: boolean;
    sharing: boolean;
    settingsOpen: boolean;
    deleteCourseOpen: boolean;
    onDeleteCourse: () => void;
  }

  let {
    workspace,
    store,
    router,
    rules,
    moving,
    movingOpen = $bindable(),
    onMoveUnit,
    chapterDialog,
    chapterOpen = $bindable(),
    onSubmitChapter,
    deleting,
    deleteOpen = $bindable(),
    trash,
    trashOpen = $bindable(),
    sharing = $bindable(),
    settingsOpen = $bindable(),
    deleteCourseOpen = $bindable(),
    onDeleteCourse,
  }: Props = $props();

  const below = $derived(deleting ? withDescendants(store.units, deleting.id).size - 1 : 0);
  const isChapter = $derived(deleting?.kind === 'chapter');
  const delTitle = $derived(
    deleting?.title || (isChapter ? m.course_chapter_untitled() : m.course_unit_untitled()),
  );

  const completedByUser = $derived.by<Record<string, number>>(() => {
    const counts: Record<string, number> = {};
    for (const member of store.progressSummary?.members ?? []) {
      counts[member.userId] = member.completedUnits;
    }
    return counts;
  });

  function memberProgress(userId: string): string | undefined {
    const summary = store.progressSummary;
    const raw = completedByUser[userId];
    if (!summary || raw === undefined) return undefined;
    const total = summary.totalUnits;
    const completed = Math.min(raw, total);
    const percent = total > 0 ? Math.round((completed / total) * 100) : 0;
    return m.course_progress_rate({ percent, completed, total });
  }
</script>

<MovePageDialog
  bind:open={movingOpen}
  wiki={store}
  pageId={moving}
  {rules}
  labels={COURSE_MOVE_LABELS}
  onMove={onMoveUnit}
/>

<ChapterNameDialog
  bind:open={chapterOpen}
  mode={chapterDialog.mode}
  initial={chapterDialog.mode === 'rename' ? chapterDialog.chapter.title : ''}
  onSubmit={onSubmitChapter}
/>

<ConfirmDialog
  bind:open={deleteOpen}
  title={isChapter ? m.course_delete_chapter_title() : m.course_delete_unit_title()}
  description={isChapter
    ? below > 0
      ? m.course_delete_chapter_below({ title: delTitle, count: below })
      : m.course_delete_chapter_alone({ title: delTitle })
    : m.course_delete_unit_info({ title: delTitle })}
  confirmLabel={m.shared_delete()}
  destructive
  onConfirm={() => {
    if (deleting) void store.deleteUnit(deleting.id);
  }}
  focusAfter={() =>
    document.querySelector<HTMLElement>(
      '[data-testid="unit-tree"] [role="treeitem"][tabindex="0"]',
    )}
/>

{#if store.canEdit}
  <TrashDialog
    bind:open={trashOpen}
    {trash}
    {workspace}
    title={m.course_trash()}
    description={m.course_trash_retention({ days: workspace.trashRetentionDays })}
    untitled={m.course_unit_untitled()}
    belowLabel={(count: number) => m.course_trash_below({ count })}
    testId="unit-trash"
  />
{/if}

{#if store.course}
  <ShareDialog
    bind:open={sharing}
    {workspace}
    shareable={store}
    membersOnly={!store.canManage}
    readOnly={!store.canManage}
    {memberProgress}
    linkOf={(readToken: string) =>
      new URL(router.href({ boardId: store.courseId, readToken }), location.href).href}
  />
  <CourseSettingsDialog
    bind:open={settingsOpen}
    course={store.course}
    onSave={(looks: CourseLooks) => void store.changeCourse(looks)}
  />
{/if}

<ConfirmDialog
  bind:open={deleteCourseOpen}
  title={m.shell_delete_course_title()}
  description={m.shell_delete_course_info({
    title: store.course?.title || m.course_untitled(),
  })}
  confirmLabel={m['DeleteBoardDialog.confirm-delete']()}
  destructive
  onConfirm={onDeleteCourse}
/>
