<!--
  A course of the example workspace: chapters and units (depth 2) with a long
  title, a unit shown, chapters open; or an empty course; or read-only; or with
  a headed body and outline; or inside a 390px narrow frame.
-->
<script lang="ts">
  import { onMount, untrack, type Snippet } from 'svelte';

  import BodyView from '$lib/editor/BodyView.svelte';
  import CourseScreen from '$lib/features/course/CourseScreen.svelte';
  import OutlineButton from '$lib/features/wiki/OutlineButton.svelte';
  import OutlineColumns from '$lib/features/wiki/OutlineColumns.svelte';
  import { PageOutline } from '$lib/features/wiki/page-outline.svelte.ts';
  import PageTree from '$lib/features/wiki/PageTree.svelte';
  import { COURSE_TREE_LABELS } from '$lib/features/wiki/tree-actions.ts';
  import { TreeOpen } from '$lib/features/wiki/tree-open.svelte.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { pane } from '$lib/layout/room.svelte.ts';
  import { courseTreeRules } from '$lib/logic/page-moves.ts';
  import { ancestorsOf, firstUnit } from '$lib/logic/page-tree.ts';
  import { UnitOpenings } from '$lib/features/course/unit-openings.svelte.ts';
  import { CourseStore, type CourseUnit, type UnitStore } from '$lib/state/index.ts';
  import { MemoryBackend } from '$lib/state/memory/backend.ts';
  import { Button } from '$lib/ui/button/index.js';

  import { exampleWorkspace } from './workspace.ts';

  let {
    empty = false,
    readOnly = false,
    manager = false,
    outlined = false,
    narrow = false,
  }: {
    empty?: boolean;
    readOnly?: boolean;
    manager?: boolean;
    outlined?: boolean;
    narrow?: boolean;
  } = $props();

  /** A body of sections, a heading and a paragraph each, in three levels. */
  const HEADED = {
    type: 'doc',
    content: [
      [1, 'Course overview · 학습 개요'],
      [2, 'Workspace setup · 워크스페이스 준비'],
      [3, 'Joining a course · 학습 참여'],
      [3, 'Reading units · 단원 읽기'],
      [2, 'Chapters and units · 장과 단원'],
      [
        3,
        'A heading long enough to wrap onto a second line of the outline · 목차에서 두 줄로 넘어가는 긴 제목',
      ],
      [1, 'Next steps · 다음 단계'],
      [2, 'Practice · 연습'],
    ].flatMap(([level, title]) => [
      { type: 'heading', attrs: { level }, content: [{ type: 'text', text: title }] },
      {
        type: 'paragraph',
        content: [
          {
            type: 'text',
            text: 'Words of the unit section, enough to fill several lines. '.repeat(8),
          },
        ],
      },
    ]),
  };

  const uid = $props.id();
  const example = exampleWorkspace();
  const rules = courseTreeRules<CourseUnit>();
  const outline = new PageOutline();
  let course = $state.raw<CourseStore | undefined>(undefined);
  let shown = $state<string | undefined>(undefined);
  const treeOpen = $derived(new TreeOpen('', course?.courseId ?? '', 'fvoci:course-tree:'));

  async function seed(): Promise<{ store: CourseStore; first?: string }> {
    const author = new MemoryBackend(example.world, 'example-member');
    const courses = author.course;
    const made = await courses.createCourse(example.world.workspace.id, {
      title: 'Workspace Onboarding · 시작 가이드',
      visibility: 'private',
    });
    await author.addMember(
      made.id,
      'example-reader',
      readOnly ? 'viewer' : manager ? 'admin' : 'editor',
    );
    let first: string | undefined;
    if (!empty) {
      const ch1 = await courses.createUnit(made.id, {
        kind: 'chapter',
        title: '1. Foundations · 기본 익히기',
      });
      first = (
        await courses.createUnit(made.id, {
          kind: 'unit',
          title: 'Welcome · 환영합니다',
          parentId: ch1.id,
        })
      ).id;
      await courses.createUnit(made.id, {
        kind: 'unit',
        title: 'Your first week · 첫 주 안내',
        parentId: ch1.id,
      });
      const ch2 = await courses.createUnit(made.id, {
        kind: 'chapter',
        title: '2. Working together · 함께 일하기',
      });
      await courses.createUnit(made.id, {
        kind: 'unit',
        title: 'A unit with a long title that does not fit · 한 줄에 다 들어가지 않는 긴 단원 제목',
        parentId: ch2.id,
      });
      await courses.createUnit(made.id, {
        kind: 'unit',
        title: 'Reference checklist · 참고 목록',
      });
    }
    await example.workspace.load();
    const store = new CourseStore(example.workspace, made.id);
    await store.load();
    return { store, first };
  }

  onMount(() => {
    let store: CourseStore | undefined;
    void seed().then((ready) => {
      store = ready.store;
      course = ready.store;
      shown = ready.first ?? firstUnit(ready.store.units)?.id;
    });
    return () => {
      store?.close();
      example.workspace.close();
    };
  });

  $effect(() => {
    const store = course;
    const id = shown;
    if (store) {
      untrack(() => {
        store.showUnit(id);
        if (id) treeOpen.set(ancestorsOf(store.units, id), true);
      });
    }
  });

  const openings = new UnitOpenings();
</script>

<div
  class="@container/pane h-[36rem] {narrow ? 'w-[390px] border border-border' : ''}"
  data-testid="course-example"
  {@attach (element: HTMLElement) => pane.attach(element)}
>
  {#if course?.status === 'ready'}
    {@const store = course}
    <CourseScreen
      course={store}
      unit={store.openUnit}
      {openings}
      onCreateFirstUnit={() => undefined}
      onEditCourse={() => undefined}
      onDeleteCourse={() => undefined}
      onOpenTrash={() => undefined}
      showBar={outline.mode === 'button'}
      stickyBar={outline.mode === 'button'}
      scrollAttach={outline.follow}
      barAttach={outline.bar}
    >
      {#snippet tree({ closeDrawer }: { closeDrawer: () => void })}
        <PageTree
          wiki={store}
          currentId={store.openUnit?.unitId}
          open={treeOpen}
          pageHref={(id: string) => `#${id}`}
          onOpenPage={(id: string) => {
            closeDrawer();
            shown = id;
          }}
          onCreate={() => undefined}
          {rules}
          labels={COURSE_TREE_LABELS}
          testId="unit-tree"
        >
          {#snippet headerActions()}
            <div class="flex shrink-0 items-center gap-1">
              <Button
                variant="ghost"
                size="sm"
                class="h-7 px-2 text-xs text-muted-foreground roomy:min-h-touch"
                data-testid="new-chapter"
              >
                {m.course_new_chapter()}
              </Button>
              <Button
                variant="ghost"
                size="sm"
                class="h-7 px-2 text-xs text-muted-foreground roomy:min-h-touch"
                data-testid="new-unit"
              >
                {m.course_new_unit()}
              </Button>
            </div>
          {/snippet}
        </PageTree>
      {/snippet}
      {#snippet outlineButton()}
        {#if outline.mode === 'button'}
          <OutlineButton {outline} id="{uid}-outline" />
        {/if}
      {/snippet}
      {#snippet outlineWrap(content: Snippet)}
        <OutlineColumns {outline} id="{uid}-outline">
          {@render content()}
        </OutlineColumns>
      {/snippet}
      {#snippet missingUnit()}
        <p>{m.app_problem_load()}</p>
      {/snippet}
      {#snippet body(unitStore: UnitStore)}
        <div {@attach outline.source}>
          <BodyView doc={outlined ? HEADED : unitStore.body} resolveUser={() => ''}>
            {#snippet empty()}
              <p class="text-muted-foreground">{m.course_unit_body_empty()}</p>
            {/snippet}
          </BodyView>
        </div>
      {/snippet}
    </CourseScreen>
  {:else}
    <p role="status">{m.app_loading()}</p>
  {/if}
</div>
