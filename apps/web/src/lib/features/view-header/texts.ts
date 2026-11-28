/** The words and icons the parts of the header share. */
import ArrowUpDownIcon from '@lucide/svelte/icons/arrow-up-down';
import CalendarDaysIcon from '@lucide/svelte/icons/calendar-days';
import ChartGanttIcon from '@lucide/svelte/icons/chart-gantt';
import EyeIcon from '@lucide/svelte/icons/eye';
import GitForkIcon from '@lucide/svelte/icons/git-fork';
import LayersIcon from '@lucide/svelte/icons/layers';
import LayoutGridIcon from '@lucide/svelte/icons/layout-grid';
import SquareKanbanIcon from '@lucide/svelte/icons/square-kanban';
import TableIcon from '@lucide/svelte/icons/table-2';
import type { Component } from 'svelte';

import type { MenuAction } from '$lib/components/menu/actions.ts';
import { m } from '$lib/i18n/paraglide/messages.js';
import type { FilterConditionMessages } from '$lib/logic/filter-conditions.ts';
import { DONE_FILTER_ID } from '$lib/logic/filter-done.ts';
import type { FilterPeriod } from '$lib/logic/filter-period.ts';
import type { QuickFilterId } from '$lib/logic/quick-filter.ts';
import {
  TITLE_COLUMN_ID,
  type BoardView,
  type PropertyTemplate,
  type ViewType,
} from '$lib/logic/wire.ts';

import { TITLE_FILTER_ID } from './filter-edit.ts';
import type { AddableViewType } from './view-fields.ts';

/** A Lucide icon. `data-icon` is a button's hint of where in it the icon sits. */
export type Icon = Component<{ class?: string; 'data-icon'?: string }>;

export const VIEW_ICONS: Record<ViewType, Icon> = {
  board: SquareKanbanIcon,
  table: TableIcon,
  gallery: LayoutGridIcon,
  calendar: CalendarDaysIcon,
  gantt: ChartGanttIcon,
};

/** The title of a view, or the words for one without a title. */
export function viewTitle(view: BoardView): string {
  return view.title || m['Sidebar.untitled-view']();
}

/** The line that adds a view of a kind, in the list of views. */
export const ADD_VIEW_LABELS: Record<AddableViewType, () => string> = {
  board: m.viewheader_add_board_view,
  table: m.viewheader_add_table_view,
  gallery: m.viewheader_add_gallery_view,
  calendar: m.viewheader_add_calendar_view,
  gantt: m.viewheader_add_gantt_view,
};

/** The title a new view of a kind starts with. */
export const NEW_VIEW_TITLES: Record<AddableViewType, () => string> = {
  board: m['View.NewBoardTitle'],
  table: m['View.NewTableTitle'],
  gallery: m['View.NewGalleryTitle'],
  calendar: m['View.NewCalendarTitle'],
  gantt: m.viewheader_new_gantt_title,
};

/** The names of the conditions of a filter. */
export const CONDITION_NAMES: FilterConditionMessages = {
  includes: m['Filter.includes'],
  notIncludes: m['Filter.not-includes'],
  isEmpty: m['Filter.is-empty'],
  isNotEmpty: m['Filter.is-not-empty'],
  isSet: m['Filter.is-set'],
  isNotSet: m['Filter.is-not-set'],
  is: m['Filter.is'],
  contains: m['Filter.contains'],
  notContains: m['Filter.not-contains'],
  startsWith: m['Filter.starts-with'],
  notStartsWith: m['Filter.not-starts-with'],
  endsWith: m['Filter.ends-with'],
  notEndsWith: m['Filter.not-ends-with'],
  isBefore: m['Filter.is-before'],
  isAfter: m['Filter.is-after'],
  isWithin: m.filter_is_within,
};

/** The names of the conditions on whether a card is done: "is set" is done. */
const DONE_CONDITION_NAMES: FilterConditionMessages = {
  ...CONDITION_NAMES,
  isSet: m.viewheader_filter_done_yes,
  isNotSet: m.viewheader_filter_done_no,
};

/** The names of the conditions of a filter on a property, or on whether a card is done. */
export function conditionNamesOf(propertyId: string): FilterConditionMessages {
  return propertyId === DONE_FILTER_ID ? DONE_CONDITION_NAMES : CONDITION_NAMES;
}

/** The names of the periods a date condition can be within. */
export const PERIOD_NAMES: Record<FilterPeriod, () => string> = {
  thisWeek: m.filter_period_this_week,
  nextWeek: m.filter_period_next_week,
  lastWeek: m.filter_period_last_week,
  thisMonth: m.filter_period_this_month,
  next7Days: m.filter_period_next_7_days,
  next30Days: m.filter_period_next_30_days,
  last7Days: m.filter_period_last_7_days,
  last30Days: m.filter_period_last_30_days,
};

/** The names of the quick filters. */
export const QUICK_FILTER_NAMES: Record<QuickFilterId, () => string> = {
  mine: m.viewheader_quick_mine,
  incomplete: m.viewheader_quick_incomplete,
  overdue: m.viewheader_quick_overdue,
  dueThisWeek: m.viewheader_quick_due_this_week,
};

/**
 * The name of what a condition or a sort is on: a property of the board,
 * the title of the card, or whether it is done. An identifier nothing
 * answers to is a property that was deleted since.
 */
export function propertyName(propertyId: string, templates: readonly PropertyTemplate[]): string {
  if (propertyId === TITLE_FILTER_ID || propertyId === TITLE_COLUMN_ID) {
    return m['default-properties.title']();
  }
  if (propertyId === DONE_FILTER_ID) return m.viewheader_filter_done();
  return (
    templates.find((template) => template.id === propertyId)?.name ??
    m.viewheader_unknown_property()
  );
}

/** The tools of the header that "…" can open. */
export type AsideTool = 'sort' | 'display-by' | 'dependencies' | 'group' | 'properties';

/** What the header offers for the view it shows; a tool that is not offered is undefined. */
export interface AsideOffer {
  /** The whole header is one row: the tools have buttons of their own, but for "dependencies". */
  row: boolean;
  /** How many sorts the view has. */
  sort: number | undefined;
  /** The name of what each list has chosen. */
  displayBy: string | undefined;
  dependencies: string | undefined;
  group: string | undefined;
}

/**
 * A name as the end of a line of a menu: cut short where it is long. The
 * menu does not shorten what stands at the end of a line, and a long name
 * there leaves no room for what the line is.
 */
function brief(name: string): string {
  const letters = Array.from(name);
  return letters.length > HINT_LETTERS ? `${letters.slice(0, HINT_LETTERS - 1).join('')}…` : name;
}
const HINT_LETTERS = 20;

/**
 * The lines of "…" for the tools that have no button of their own: in the
 * folded row most of them; in the whole row "dependencies" alone (set once
 * for a Gantt chart and seldom again, it takes no place in the row).
 */
export function asideTools(offer: AsideOffer, open: Record<AsideTool, () => void>): MenuAction[] {
  const line = (id: AsideTool, label: string, icon: Icon, hint?: string): MenuAction => ({
    id,
    label,
    icon,
    ...(hint === undefined ? {} : { hint: brief(hint) }),
    run: open[id],
  });
  const folded = !offer.row;
  const sorts = offer.sort ? String(offer.sort) : undefined;
  return [
    folded &&
      offer.sort !== undefined &&
      line('sort', m['ViewHeader.sort'](), ArrowUpDownIcon, sorts),
    folded &&
      offer.displayBy !== undefined &&
      line('display-by', m.viewheader_display_by(), CalendarDaysIcon, offer.displayBy),
    offer.dependencies !== undefined &&
      line('dependencies', m.viewheader_dependencies(), GitForkIcon, offer.dependencies),
    folded &&
      offer.group !== undefined &&
      line('group', m.viewheader_group_by(), LayersIcon, offer.group),
    folded && line('properties', m['ViewHeader.properties'](), EyeIcon),
  ].filter((each) => each !== false);
}
