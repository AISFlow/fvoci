<!--
  Horizontal WAI-ARIA tab strip for switching a course between Units,
  Study Meetings, and Exams. A tab with nothing in it is left out unless it is
  the one shown: the course menu makes the first meeting or exam. Supports
  keyboard ArrowLeft/ArrowRight/Home/End navigation and compact wrapping on
  390px screens. Never mounted on public share links (`/shared/...`).
-->
<script lang="ts">
  import BookOpenIcon from '@lucide/svelte/icons/book-open';
  import ClipboardCheckIcon from '@lucide/svelte/icons/clipboard-check';
  import UsersIcon from '@lucide/svelte/icons/users';

  import { m } from '$lib/i18n/paraglide/messages.js';

  import type { CourseSectionTab } from './course-meeting-labels.ts';

  interface Props {
    active: CourseSectionTab;
    meetingsCount: number;
    examsCount: number;
    panelIdPrefix: string;
    onSelect: (tab: CourseSectionTab) => void;
  }

  let { active, meetingsCount, examsCount, panelIdPrefix, onSelect }: Props = $props();

  let listElement: HTMLElement | undefined;

  const all = $derived([
    { id: 'units' as const, label: m.course_tab_units(), count: 0, icon: BookOpenIcon },
    {
      id: 'meetings' as const,
      label: m.course_tab_meetings(),
      count: meetingsCount,
      icon: UsersIcon,
    },
    {
      id: 'exams' as const,
      label: m.course_tab_exams(),
      count: examsCount,
      icon: ClipboardCheckIcon,
    },
  ]);
  const tabs = $derived(
    all.filter((tab) => tab.id === 'units' || tab.count > 0 || tab.id === active),
  );
  const order = $derived(tabs.map((tab) => tab.id));

  function focusAndSelect(next: CourseSectionTab): void {
    onSelect(next);
    const button = listElement?.querySelector<HTMLButtonElement>(`[data-tab="${next}"]`);
    button?.focus();
  }

  function onkeydown(event: KeyboardEvent): void {
    const index = order.indexOf(active);
    if (index === -1) return;
    const moves: Partial<Record<string, number>> = {
      ArrowRight: (index + 1) % order.length,
      ArrowLeft: (index - 1 + order.length) % order.length,
      Home: 0,
      End: order.length - 1,
    };
    const at = moves[event.key];
    const next = at === undefined ? undefined : order[at];
    if (!next) return;
    event.preventDefault();
    focusAndSelect(next);
  }
</script>

<div
  bind:this={listElement}
  role="tablist"
  aria-label={m.course_tabs_label()}
  data-testid="course-tabs"
  class="flex min-w-0 items-center gap-1 overflow-x-auto"
>
  {#each tabs as tab (tab.id)}
    {@const selected = active === tab.id}
    {@const Icon = tab.icon}
    <button
      type="button"
      role="tab"
      id="{panelIdPrefix}-tab-{tab.id}"
      aria-selected={selected}
      aria-controls={selected ? `${panelIdPrefix}-panel-${tab.id}` : undefined}
      tabindex={selected ? 0 : -1}
      data-tab={tab.id}
      data-testid="course-tab-{tab.id}"
      class={[
        'inline-flex shrink-0 items-center gap-1.5 rounded-md px-2.5 py-1.5 text-sm font-medium transition-colors roomy:min-h-touch',
        selected
          ? 'bg-accent text-accent-foreground shadow-xs'
          : 'text-muted-foreground hover:bg-muted hover:text-foreground',
      ]}
      onclick={() => {
        onSelect(tab.id);
      }}
      {onkeydown}
    >
      <Icon class="size-4 shrink-0" aria-hidden="true" />
      <span>{tab.label}</span>
      {#if tab.count > 0}
        <span
          class="rounded-full bg-muted px-1.5 text-xs text-muted-foreground tabular-nums"
          data-testid="course-tab-{tab.id}-count"
        >
          {tab.count}
        </span>
      {/if}
    </button>
  {/each}
</div>
