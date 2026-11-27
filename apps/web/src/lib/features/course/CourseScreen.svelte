<!--
  A course: its chapters and units in the left column (or behind a button as a
  drawer in a narrow pane), and the open unit in the center.

  Shared tree, outline and viewers components live in `features/wiki/` and are
  composed here via snippets from `CoursePage.svelte` so feature boundaries
  stay clean.
-->
<script lang="ts">
  import { tick, type Snippet } from 'svelte';
  import type { Attachment } from 'svelte/attachments';

  import FilePlusIcon from '@lucide/svelte/icons/file-plus';
  import ListTreeIcon from '@lucide/svelte/icons/list-tree';
  import XIcon from '@lucide/svelte/icons/x';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { closeOnBack } from '$lib/layout/back.svelte.ts';
  import { focusRevealed } from '$lib/focus.ts';
  import { coverPane } from '$lib/layout/over-pane.svelte.ts';
  import { pane } from '$lib/layout/room.svelte.ts';
  import type { CourseStore, UnitStore } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import CourseHeader from './CourseHeader.svelte';
  import CourseExamDialog from './CourseExamDialog.svelte';
  import CourseMeetingDialog from './CourseMeetingDialog.svelte';
  import CourseTabBar from './CourseTabBar.svelte';
  import type { CourseSectionTab } from './course-meeting-labels.ts';
  import type { UnitOpenings } from './unit-openings.svelte.ts';
  import UnitView from './UnitView.svelte';

  interface Props {
    course: CourseStore;
    unit: UnitStore | undefined;
    /** What the unit's menu opens below its head. */
    openings: UnitOpenings;
    /** Active section tab of the course (`units` by default). */
    activeTab?: CourseSectionTab;
    onSelectTab?: ((tab: CourseSectionTab) => void) | undefined;
    meetingsTab?: Snippet | undefined;
    examsTab?: Snippet | undefined;
    /** Whether the unit shown was just created here (autofocus its title). */
    autofocusUnit?: boolean;
    /** Whether a unit/chapter creation is currently in flight. */
    creating?: boolean;
    /** Creates the first unit from the empty state. */
    onCreateFirstUnit: () => void;
    onEditCourse: () => void;
    onDeleteCourse: () => void;
    onOpenTrash: () => void;
    onShare?: (() => void) | undefined;
    /** Optional reserved slot for L2 "My progress". */
    progress?: Snippet | undefined;
    /** The outline tree rendered in the left column; receives a callback to close a narrow drawer. */
    tree: Snippet<[{ closeDrawer: (restoreFocus?: boolean) => void }]>;
    /** Sticky bar outline button when headings live behind a button. */
    outlineButton?: Snippet | undefined;
    /** True when the sticky bar should stay visible or keep its space for the outline button. */
    showBar?: boolean;
    /** True when the sticky bar has visible controls and should stick on scroll. */
    stickyBar?: boolean;
    /** Attachment for the scrollable content column (tracks outline scroll position). */
    scrollAttach?: Attachment<HTMLElement> | undefined;
    /** Attachment for the sticky bar (measures its height for outline scroll offset). */
    barAttach?: Attachment<HTMLElement> | undefined;
    /** Wraps the open unit view (e.g. `OutlineColumns`). */
    outlineWrap?: Snippet<[Snippet]> | undefined;
    /** Live viewers badge for the unit heading. */
    viewers?: Snippet<[string]> | undefined;
    /** Unit head actions (e.g. versions menu). */
    unitActions?: Snippet<[UnitStore]> | undefined;
    /** Shown when a unit URL names a missing unit. */
    missingUnit: Snippet;
    /** The unit body editor. */
    body: Snippet<[UnitStore]>;
    /** Notice shown at the head of the unit (e.g. deleted by another user). */
    notice?: string | undefined;
    onDismissNotice?: (() => void) | undefined;
    takeFocus?: boolean;
  }

  let {
    course,
    unit,
    openings,
    activeTab = 'units',
    onSelectTab,
    meetingsTab,
    examsTab,
    autofocusUnit = false,
    creating = false,
    onCreateFirstUnit,
    onEditCourse,
    onDeleteCourse,
    onOpenTrash,
    onShare,
    progress,
    tree,
    outlineButton,
    showBar = false,
    stickyBar = false,
    scrollAttach,
    barAttach,
    outlineWrap,
    viewers,
    unitActions,
    missingUnit,
    body,
    notice,
    onDismissNotice,
    takeFocus = false,
  }: Props = $props();

  const uid = $props.id();
  const shown = $derived(unit ? course.unit(unit.unitId) : undefined);
  const hasUnits = $derived(course.units.some((item) => item.kind !== 'chapter'));
  const reading = $derived(
    hasUnits && !unit?.missing && shown !== undefined && shown.kind !== 'chapter',
  );
  const narrow = $derived(pane.measured && !pane.columns);
  let makingMeeting = $state(false);
  let makingExam = $state(false);

  /** Shows the tab of `tab` with the dialog that makes one more of it. */
  function startEvent(tab: 'meetings' | 'exams'): void {
    onSelectTab?.(tab);
    if (tab === 'meetings') makingMeeting = true;
    else makingExam = true;
  }

  /** Tabs to choose between: a meeting or an exam, or a tab of them shown (`CourseTabBar`). */
  const tabbed = $derived(
    onSelectTab !== undefined &&
      (course.events.meetings.length > 0 ||
        course.events.exams.length > 0 ||
        activeTab !== 'units'),
  );
  const place = $derived(`${course.courseId} ${unit?.unitId ?? ''} ${activeTab}`);
  const away = (_over: string): boolean => false;
  let drawer = $derived(away(place));
  const drawerOut = $derived(narrow && drawer);
  coverPane(() => drawerOut);
  let panel = $state<HTMLElement>();
  let opener = $state<HTMLElement | null>(null);

  async function openDrawer(): Promise<void> {
    drawer = true;
    await tick();
    const target =
      panel?.querySelector<HTMLElement>('[role="treeitem"][tabindex="0"]') ??
      panel?.querySelector<HTMLElement>('button');
    focusRevealed(target);
  }

  async function closeDrawer(restoreFocus = true): Promise<void> {
    drawer = false;
    if (!restoreFocus) return;
    await tick();
    focusRevealed(opener);
  }

  closeOnBack(
    () => drawerOut,
    () => {
      drawer = false;
    },
  );

  const stops = (within: HTMLElement): HTMLElement[] =>
    Array.from(
      within.querySelectorAll<HTMLElement>(
        'button:not([tabindex="-1"]):not(:disabled), a[href]:not([tabindex="-1"]), [tabindex="0"]',
      ),
    );

  function onkeydown(event: KeyboardEvent): void {
    if (!drawerOut || event.defaultPrevented || !panel) return;
    if (event.key === 'Escape') {
      event.preventDefault();
      void closeDrawer();
    } else if (event.key === 'Tab') {
      const inside = stops(panel);
      const [first, last] = [inside[0], inside.at(-1)];
      const at = document.activeElement;
      if (!event.shiftKey && at === last) {
        event.preventDefault();
        first?.focus();
      } else if (event.shiftKey && at === first) {
        event.preventDefault();
        last?.focus();
      }
    }
  }

  const noopAttach: Attachment<HTMLElement> = () => undefined;

  /**
   * The meetings and the exams read again as the reader comes back to the window: the
   * server tells nobody of them, so the first one somebody else made (and the tab it brings)
   * shows here then.
   */
  function readEventsAgain(): void {
    if (document.visibilityState === 'visible') void course.events.refreshAll();
  }
</script>

<svelte:window onfocus={readEventsAgain} />
<svelte:document onvisibilitychange={readEventsAgain} />

<div
  class="relative flex h-full min-h-0 outline-none"
  tabindex="-1"
  data-testid="course-screen"
  data-narrow={narrow ? '' : undefined}
  {@attach (element: HTMLElement) => {
    if (takeFocus) element.focus();
  }}
>
  {#if drawerOut}
    <button
      type="button"
      tabindex="-1"
      aria-label={m.course_close_tree()}
      data-testid="unit-tree-scrim"
      class="absolute inset-0 z-20 animate-in cursor-default scrim duration-200 fade-in-0"
      onclick={() => void closeDrawer()}
    ></button>
  {/if}

  <div
    id="{uid}-tree"
    bind:this={panel}
    data-testid="unit-tree-panel"
    role={drawerOut ? 'dialog' : undefined}
    aria-modal={drawerOut ? 'true' : undefined}
    aria-label={drawerOut ? m.course_units() : undefined}
    inert={narrow && !drawer}
    class={[
      'flex min-h-0 flex-col bg-background',
      narrow
        ? 'absolute inset-y-0 left-0 z-30 w-[min(18rem,85%)] border-r shadow-lg'
        : 'w-64 shrink-0 border-r',
      narrow &&
        (drawer
          ? '[transition:translate_200ms_ease-out]'
          : 'invisible -translate-x-full [transition:translate_200ms_ease-in,visibility_0s_200ms]'),
    ]}
    {onkeydown}
  >
    {#if drawerOut}
      <div class="flex shrink-0 justify-end px-1 pt-1">
        <IconButton label={m.course_close_tree()} onclick={() => void closeDrawer()}>
          <XIcon />
        </IconButton>
      </div>
    {/if}
    <CourseHeader
      {course}
      onEdit={onEditCourse}
      onDelete={onDeleteCourse}
      onTrash={onOpenTrash}
      {onShare}
      onNewMeeting={onSelectTab
        ? () => {
            startEvent('meetings');
          }
        : undefined}
      onNewExam={onSelectTab
        ? () => {
            startEvent('exams');
          }
        : undefined}
      {progress}
    />
    {@render tree({
      closeDrawer: (restoreFocus = true) => {
        if (drawerOut) void closeDrawer(restoreFocus);
      },
    })}
  </div>

  <div
    class="min-w-0 flex-1 overflow-y-auto overscroll-contain"
    inert={drawerOut}
    {@attach scrollAttach ?? noopAttach}
  >
    {#if narrow || showBar || tabbed}
      <div
        class={[
          'top-0 z-10 flex min-h-10 items-center gap-2 border-b bg-background/95 px-2 py-1 roomy:min-h-[calc(var(--spacing-touch)+0.5rem)]',
          (narrow || stickyBar || tabbed) && 'sticky',
        ]}
        {@attach barAttach ?? noopAttach}
      >
        {#if narrow}
          <Button
            bind:ref={opener}
            variant="ghost"
            size="sm"
            class="shrink-0 text-muted-foreground roomy:min-h-touch"
            data-testid="open-unit-tree"
            aria-haspopup="dialog"
            aria-expanded={drawer}
            aria-controls="{uid}-tree"
            onclick={() => void openDrawer()}
          >
            <ListTreeIcon aria-hidden="true" />
            {m.course_units()}
          </Button>
        {/if}
        {#if tabbed && onSelectTab}
          <CourseTabBar
            active={activeTab}
            meetingsCount={course.events.meetings.length}
            examsCount={course.events.exams.length}
            panelIdPrefix={uid}
            onSelect={onSelectTab}
          />
        {/if}
        <div class="min-w-0 flex-1"></div>
        {#if activeTab === 'units'}
          {@render outlineButton?.()}
        {/if}
      </div>
    {/if}

    {#if activeTab === 'meetings' && meetingsTab}
      <div role="tabpanel" id="{uid}-panel-meetings" aria-labelledby="{uid}-tab-meetings">
        {@render meetingsTab()}
      </div>
    {:else if activeTab === 'exams' && examsTab}
      <div role="tabpanel" id="{uid}-panel-exams" aria-labelledby="{uid}-tab-exams">
        {@render examsTab()}
      </div>
    {:else}
      <div
        role={tabbed ? 'tabpanel' : undefined}
        id="{uid}-panel-units"
        aria-labelledby={tabbed ? `${uid}-tab-units` : undefined}
        class="h-full"
      >
        {#if notice && !reading}{@render told(notice)}{/if}
        {#if !hasUnits && !unit?.missing}
          <div class="grid h-full place-items-center p-6 text-center" data-testid="course-empty">
            <div class="flex max-w-md flex-col items-center gap-3">
              <p class="text-muted-foreground">{m.course_no_units()}</p>
              {#if course.canEdit}
                <Button
                  class="roomy:min-h-touch"
                  data-testid="course-first-unit"
                  disabled={creating}
                  onclick={onCreateFirstUnit}
                >
                  <FilePlusIcon aria-hidden="true" />
                  {m.course_first_unit()}
                </Button>
              {/if}
            </div>
          </div>
        {:else if unit?.missing}
          {@render missingUnit()}
        {:else if unit && shown && shown.kind !== 'chapter'}
          {@const openUnit = unit}
          {@const shownUnit = shown}
          {#snippet unitContent()}
            {#if notice}{@render told(notice)}{/if}
            {#key openUnit}
              {#snippet unitViewers()}
                {@render viewers?.(shownUnit.id)}
              {/snippet}
              <UnitView
                {course}
                store={openUnit}
                unit={shownUnit}
                {openings}
                autofocus={autofocusUnit}
                viewers={viewers ? unitViewers : undefined}
                actions={unitActions}
                {body}
              />
            {/key}
          {/snippet}
          {#if outlineWrap}
            {@render outlineWrap(unitContent)}
          {:else}
            {@render unitContent()}
          {/if}
        {/if}
      </div>
    {/if}
  </div>
</div>

{#if onSelectTab && course.canEdit}
  <CourseMeetingDialog bind:open={makingMeeting} {course} />
  <CourseExamDialog bind:open={makingExam} {course} />
{/if}

{#snippet told(text: string)}
  <div
    class="mx-auto mt-4 flex max-w-3xl items-center gap-2 rounded-lg border bg-muted px-3 py-2"
    role="status"
    data-testid="course-notice"
  >
    <p class="min-w-0 flex-1">{text}</p>
    {#if onDismissNotice}
      <IconButton label={m.ui_close()} onclick={onDismissNotice}>
        <XIcon />
      </IconButton>
    {/if}
  </div>
{/snippet}
