/**
 * Localized labels, badge styles, bar colors, and human-readable duration
 * formatting for personal time tracking UI components.
 */
import { m } from '$lib/i18n/paraglide/messages.js';
import { durationParts, type TimeEntryKind } from '$lib/logic/time.ts';
import type { TimeEntry } from '$lib/state/index.ts';

/** Localized display label for a `TimeEntryKind`. */
export function kindLabel(kind: TimeEntryKind): string {
  switch (kind) {
    case 'lecture':
      return m.time_kind_lecture();
    case 'study':
      return m.time_kind_study();
    case 'work':
      return m.time_kind_work();
    case 'other':
      return m.time_kind_other();
  }
}

/** Contrast-safe badge classes for light and dark themes (using app design tokens). */
export const KIND_BADGE_CLASS: Record<TimeEntryKind, string> = {
  lecture: 'bg-option-blue text-foreground border-border',
  study: 'bg-option-green text-foreground border-border',
  work: 'bg-option-orange text-foreground border-border',
  other: 'bg-option-default text-foreground border-border',
};

/** Swatch classes for summary bar segments and legend dots (using app design tokens). */
export const KIND_BAR_CLASS: Record<TimeEntryKind, string> = {
  lecture: 'bg-option-blue',
  study: 'bg-option-green',
  work: 'bg-option-orange',
  other: 'bg-option-gray',
};

/** Formats milliseconds into a compact localized duration (`2h 15m`, `45m`, `12s`, `0m`). */
export function formatHumanDuration(ms: number): string {
  const { hours, minutes, seconds } = durationParts(ms);
  if (hours > 0 && minutes > 0) return m.time_duration_hm({ hours, minutes });
  if (hours > 0) return m.time_duration_h({ hours });
  if (minutes > 0) return m.time_duration_m({ minutes });
  if (seconds > 0) return m.time_duration_s({ seconds });
  return m.time_duration_zero();
}

/** Display title for an entry's linked target, falling back gracefully when untitled or targetless. */
export function entryTargetTitle(entry: TimeEntry): string | null {
  if (entry.targetTitle && entry.targetTitle.trim() !== '') return entry.targetTitle;
  switch (entry.targetKind) {
    case 'card':
      return m['KanbanCard.untitled']();
    case 'unit':
      return m.course_unit_untitled();
    case 'board':
      return m['Sidebar.untitled-board']();
    case 'course':
      return m.course_untitled();
    default:
      return null;
  }
}

/** Label shown on the global active-timer pill (target title, or kind label when targetless). */
export function pillTargetLabel(entry: TimeEntry): string {
  return entryTargetTitle(entry) ?? kindLabel(entry.kind);
}
