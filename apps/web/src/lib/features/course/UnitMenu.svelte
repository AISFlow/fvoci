<!--
  The menu at the head of a unit: what can be done with the unit shown. Sets
  its amount and dates and plans a review of it (`openings`), which stay out of
  the unit's head until they are set, copies its link (the address of the page,
  which shows the unit: a body pastes it as a reference to the unit), and opens
  the versions kept of the unit's body (`onVersions`).
-->
<script lang="ts">
  import CalendarCheckIcon from '@lucide/svelte/icons/calendar-check';
  import CalendarClockIcon from '@lucide/svelte/icons/calendar-clock';
  import EllipsisIcon from '@lucide/svelte/icons/ellipsis';
  import HistoryIcon from '@lucide/svelte/icons/history';
  import LinkIcon from '@lucide/svelte/icons/link';

  import { copyCardLink } from '$lib/components/card-number/card-link.ts';
  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import ActionMenu from '$lib/components/menu/ActionMenu.svelte';
  import type { MenuAction } from '$lib/components/menu/actions.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { CourseStore, CourseUnit } from '$lib/state/index.ts';

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
  /** Said for a while once the link is copied. */
  let copied = $state(false);

  async function copyLink(): Promise<void> {
    // The address of the page shown, which shows the unit, without what it was reached by.
    const address = `${location.origin}${location.pathname}`;
    try {
      await copyCardLink(address, { title });
    } catch {
      // No clipboard here (a page that is not served securely): nothing was copied, so nothing is said.
      return;
    }
    copied = true;
    setTimeout(() => {
      copied = false;
    }, 2000);
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
    { id: 'versions', label: m.card_versions(), icon: HistoryIcon, run: onVersions },
  ]);
</script>

<span class="text-xs text-muted-foreground" role="status" data-testid="unit-link-copied">
  {copied ? m['CardActionsMenu.copiedLink']() : ''}
</span>

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
