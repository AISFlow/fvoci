<!--
  The menu at the head of a unit: what can be done with the unit shown. Sets
  its amount and dates and plans a review of it (`openings`), which stay out of
  the unit's head until they are set, copies its link (the address of the page,
  which shows the unit: a body pastes it as a reference to the unit), copies or
  saves its body as Markdown (not through a public link), and opens the
  versions kept of the unit's body (`onVersions`).
-->
<script lang="ts">
  import CalendarCheckIcon from '@lucide/svelte/icons/calendar-check';
  import CalendarClockIcon from '@lucide/svelte/icons/calendar-clock';
  import EllipsisIcon from '@lucide/svelte/icons/ellipsis';
  import HistoryIcon from '@lucide/svelte/icons/history';
  import LinkIcon from '@lucide/svelte/icons/link';

  import { copyCardLink } from '$lib/components/card-number/card-link.ts';
  import FavoriteToggle from '$lib/components/favorite-toggle/FavoriteToggle.svelte';
  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import ActionMenu from '$lib/components/menu/ActionMenu.svelte';
  import type { MenuAction } from '$lib/components/menu/actions.ts';
  import { markdownActions } from '$lib/components/markdown/markdown-actions.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { workspaceFavorites, type CourseStore, type CourseUnit } from '$lib/state/index.ts';
  import { markdownReader } from '$lib/state/markdown-types.ts';

  import type { UnitOpenings } from './unit-openings.svelte.ts';

  interface Props {
    course: CourseStore;
    unit: CourseUnit;
    openings: UnitOpenings;
    /** Opens the versions kept of the unit's body. */
    onVersions: () => void;
  }

  let { course, unit, openings, onVersions }: Props = $props();

  const title = $derived(unit.title || m.course_unit_untitled());
  /** What was copied, said for a while. */
  let copied = $state<'link' | 'markdown' | undefined>();

  function say(what: 'link' | 'markdown'): void {
    copied = what;
    setTimeout(() => {
      if (copied === what) copied = undefined;
    }, 2000);
  }

  async function copyLink(): Promise<void> {
    // The address of the page shown, which shows the unit, without what it was reached by.
    const address = `${location.origin}${location.pathname}`;
    try {
      await copyCardLink(address, { title });
    } catch {
      // No clipboard here (a page that is not served securely): nothing was copied, so nothing is said.
      return;
    }
    say('link');
  }

  const actions = $derived<MenuAction[]>([
    ...(course.canEdit || course.canTrackProgress
      ? [
          {
            id: 'schedule',
            label: m.course_unit_set_schedule(),
            icon: CalendarClockIcon,
            afterClose: () => {
              openings.schedule = unit.id;
            },
          },
        ]
      : []),
    ...(course.canTrackProgress
      ? [
          {
            id: 'review',
            label: m.review_panel_add(),
            icon: CalendarCheckIcon,
            afterClose: () => {
              openings.review = unit.id;
            },
          },
        ]
      : []),
    {
      id: 'copy-link',
      label: m['CardActionsMenu.copyLink'](),
      icon: LinkIcon,
      run: () => void copyLink(),
    },
    ...markdownActions({
      read: markdownReader(course.workspace.backend.markdown, {
        kind: 'unit',
        courseId: course.courseId,
        unitId: unit.id,
      }),
      title: unit.title,
      onCopied: () => {
        say('markdown');
      },
      onRefused: (error) => {
        course.workspace.report('load', error);
      },
    }),
    { id: 'versions', label: m.card_versions(), icon: HistoryIcon, run: onVersions },
  ]);
</script>

<span class="text-xs text-muted-foreground" role="status" data-testid="unit-link-copied">
  {copied === 'link'
    ? m['CardActionsMenu.copiedLink']()
    : copied === 'markdown'
      ? m.markdown_copied()
      : ''}
</span>

{#if !course.container?.isTemplate}
  <FavoriteToggle stars={workspaceFavorites(course.workspace)} targetId={unit.id} />
{/if}

<ActionMenu title={m.course_unit_more({ title })} subtitle={title} align="end" {actions}>
  {#snippet trigger(props: Record<string, unknown>)}
    <IconButton
      {...props}
      label={m.course_unit_more({ title })}
      data-testid="unit-menu"
      data-unit-menu
    >
      <EllipsisIcon aria-hidden="true" />
    </IconButton>
  {/snippet}
</ActionMenu>
