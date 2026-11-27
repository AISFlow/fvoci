<!--
  Compact trailing study-amount and effective-date/overdue badges shown beside
  each unit row in the course outline tree (`PageTree`). Never renders on
  chapters and never mounted on public share links.
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { CourseStore, CourseUnit } from '$lib/state/index.ts';

  import { formatAmountShort, shortMonthDay } from './course-schedule-labels.ts';

  interface Props {
    course: CourseStore;
    unit: CourseUnit;
  }

  let { course, unit }: Props = $props();

  const scheduled = $derived(unit.kind === 'unit' ? course.schedule.unit(unit.id) : undefined);
  const amount = $derived(scheduled ? scheduled.amount : (unit.amount ?? null));
  const amountUnit = $derived(scheduled ? scheduled.amountUnit : (unit.amountUnit ?? null));
  const amountText = $derived(unit.kind === 'unit' ? formatAmountShort(amount, amountUnit) : '');
  const effectiveDate = $derived(
    unit.kind === 'unit'
      ? scheduled
        ? scheduled.effectiveDate
        : (unit.scheduledDate ?? null)
      : null,
  );
  const dateText = $derived(shortMonthDay(effectiveDate));
  const overdue = $derived(scheduled?.overdue ?? false);
  const isToday = $derived(
    !scheduled?.completed &&
      effectiveDate !== null &&
      course.schedule.today !== '' &&
      effectiveDate === course.schedule.today,
  );
</script>

{#if unit.kind === 'unit' && (amountText !== '' || dateText !== '')}
  <span class="flex shrink-0 items-center gap-1 text-[0.6875rem] tabular-nums">
    {#if amountText !== ''}
      <span
        class="rounded bg-muted px-1 py-0.5 text-muted-foreground"
        data-testid="unit-tree-amount"
      >
        {amountText}
      </span>
    {/if}
    {#if dateText !== ''}
      <span
        class={[
          'rounded px-1 py-0.5 font-medium',
          overdue
            ? 'bg-destructive/15 text-destructive'
            : isToday
              ? 'bg-primary/15 text-primary'
              : 'text-muted-foreground',
        ]}
        title={overdue
          ? `${effectiveDate} (${m.course_unit_badge_overdue()})`
          : isToday
            ? `${effectiveDate} (${m.course_unit_badge_today()})`
            : (effectiveDate ?? '')}
        data-testid="unit-tree-date"
        data-overdue={overdue ? 'true' : 'false'}
        data-today={isToday ? 'true' : 'false'}
      >
        {dateText}
        {#if overdue}
          <span class="sr-only"> ({m.course_unit_badge_overdue()})</span>
        {:else if isToday}
          <span class="sr-only"> ({m.course_unit_badge_today()})</span>
        {/if}
      </span>
    {/if}
  </span>
{/if}
