/**
 * What the header writes into a view: the fields of a new view, of a change
 * of grouping, of "display by" and of "dependencies", and of the properties
 * a view shows.
 *
 * Everything here is a pure function of the view and the board's property
 * definitions, so that the choices made (what a new view starts with, what a
 * change of grouping resets) are in one place and are tested without a
 * screen.
 */
import { displayableProperties, displayProperty } from '$lib/logic/calendar.ts';
import { DEFAULT_GANTT_ZOOM, dependencyProperty } from '$lib/logic/gantt.ts';
import { propertyBehavior } from '$lib/logic/properties.ts';
import {
  createBoardView,
  TITLE_COLUMN_ID,
  type BoardView,
  type PropertyTemplate,
  type ViewType,
} from '$lib/logic/wire.ts';

type ViewFields = BoardView['fields'];

/** The kinds of view that can be added. */
export const ADDABLE_VIEW_TYPES = ['board', 'table', 'gallery', 'calendar', 'gantt'] as const;
export type AddableViewType = (typeof ADDABLE_VIEW_TYPES)[number];

/** Whether a view of this kind has a grouping, and whether "no grouping" is a choice. */
export function groupingOf(viewType: ViewType): { offered: boolean; optional: boolean } {
  const optional = viewType === 'table' || viewType === 'gantt';
  return { offered: viewType === 'board' || optional, optional };
}

/** A calendar places its cards by date: it has no sort. */
export function sortOffered(viewType: ViewType): boolean {
  return viewType !== 'calendar';
}

/** The properties a view can be grouped by. */
export function groupableProperties(templates: readonly PropertyTemplate[]): PropertyTemplate[] {
  return templates.filter((template) => propertyBehavior(template.type).canGroup);
}

/**
 * The property a view is grouped by. A board view that names none is grouped
 * by the first select property, as it is drawn.
 */
export function groupProperty(
  view: BoardView,
  templates: readonly PropertyTemplate[],
): PropertyTemplate | undefined {
  const named = templates.find((template) => template.id === view.fields.groupBy);
  if (named || view.fields.viewType !== 'board') return named;
  return templates.find((template) => template.type === 'select');
}

/**
 * The fields of a view grouped by `template`, or by nothing. The columns
 * start over with the new property: the one of cards without a value first,
 * then one per option, none hidden and none collapsed. What was hidden or
 * collapsed belonged to the options of the old property.
 */
export function groupingFields(
  template: PropertyTemplate | undefined,
): Pick<ViewFields, 'groupBy' | 'visibleGroups' | 'hiddenGroups' | 'collapsedGroups'> {
  return {
    // Not grouped is the empty identifier, as the built-in templates write it.
    groupBy: template?.id ?? '',
    visibleGroups: template ? ['', ...template.options.map((option) => option.id)] : [],
    hiddenGroups: [],
    collapsedGroups: [],
  };
}

/**
 * The fields a new view starts with: a board grouped by the first select
 * property, a table that shows every property, a gallery that shows titles,
 * a calendar placed by the first date property (or the first kind of date;
 * by when cards were made where the board has none, and then it names none),
 * a Gantt chart placed the same way, not grouped, zoomed to weeks.
 * A table stores no width for its title column: until the reader sets one
 * the table gives the column what the room it has allows (table/columns.ts).
 */
export function newViewFields(
  viewType: AddableViewType,
  templates: readonly PropertyTemplate[],
): ViewFields {
  const fields: ViewFields = { ...createBoardView().fields, viewType };
  if (viewType === 'board') {
    const first = templates.find((template) => template.type === 'select');
    return first ? { ...fields, ...groupingFields(first) } : fields;
  }
  if (viewType === 'table') {
    return {
      ...fields,
      ...groupingFields(undefined),
      shownProperties: templates.map((template) => template.id),
    };
  }
  if (viewType === 'calendar' || viewType === 'gantt') {
    return {
      ...fields,
      ...(viewType === 'gantt' ? ganttFields(templates) : {}),
      shownProperties: [TITLE_COLUMN_ID],
      dateProperty: displayProperty(undefined, templates)?.id,
    };
  }
  return { ...fields, shownProperties: [TITLE_COLUMN_ID] };
}

/**
 * What a new Gantt chart has beside what a calendar has: no grouping, the
 * zoom of weeks, and the first card relation of the board as what comes
 * before what (none where the board has no card relation).
 */
function ganttFields(templates: readonly PropertyTemplate[]): Partial<ViewFields> {
  const relation = templates.find((template) => template.type === 'relation');
  return {
    ...groupingFields(undefined),
    ganttZoom: DEFAULT_GANTT_ZOOM,
    ...(relation ? { dependencyPropertyId: relation.id } : {}),
  };
}

/** A calendar and a Gantt chart are placed by a date: they have "display by". */
export function displayByOffered(viewType: ViewType): boolean {
  return viewType === 'calendar' || viewType === 'gantt';
}

/** What "display by" names: the kind of date in use, or `madeName` where the board has none. */
export function displayByName(
  view: BoardView,
  templates: readonly PropertyTemplate[],
  madeName: string,
): string {
  return displayProperty(view.fields.dateProperty, templates)?.name ?? madeName;
}

/** One line of the list "display by" or "dependencies" offers. `write`: choosing it changes the view. */
export interface DisplayByChoice {
  id: string;
  label: string;
  checked: boolean;
  write: boolean;
}

/**
 * The lines of "display by": every kind of date the board has, the one in
 * use checked. Choosing the one in use again writes nothing, unless the view
 * named none and was placed by the first one (then it names it from now on).
 * A board without any kind of date has one line, checked and inert: the
 * cards are placed by when they were made, and `madeName` says so.
 */
export function displayByChoices(
  view: BoardView,
  templates: readonly PropertyTemplate[],
  madeName: string,
): DisplayByChoice[] {
  const named = view.fields.dateProperty;
  const used = displayProperty(named, templates);
  const choices = displayableProperties(templates).map((template) => ({
    id: template.id,
    label: template.name,
    checked: template.id === used?.id,
    write: template.id !== named,
  }));
  return choices.length > 0 ? choices : [{ id: '', label: madeName, checked: true, write: false }];
}

/** A Gantt chart reads one card relation as what comes before what: it has "dependencies". */
export function dependenciesOffered(viewType: ViewType): boolean {
  return viewType === 'gantt';
}

/** What "dependencies" names: the card relation in use, or `noneName`. */
export function dependenciesName(
  view: BoardView,
  templates: readonly PropertyTemplate[],
  noneName: string,
): string {
  return dependencyProperty(view.fields.dependencyPropertyId, templates)?.name ?? noneName;
}

/**
 * The lines of "dependencies": none (the empty identifier), then every card
 * relation of the board, the one in use checked. Choosing what the view
 * names already writes nothing. A view that names a property that is gone
 * or is no card relation any more has "none" checked, and choosing it
 * clears the name.
 */
export function dependenciesChoices(
  view: BoardView,
  templates: readonly PropertyTemplate[],
  noneName: string,
): DisplayByChoice[] {
  const named = view.fields.dependencyPropertyId ?? '';
  const used = dependencyProperty(named, templates);
  return [
    { id: '', label: noneName, checked: !used, write: named !== '' },
    ...templates
      .filter((template) => template.type === 'relation')
      .map((template) => ({
        id: template.id,
        label: template.name,
        checked: template.id === used?.id,
        write: template.id !== named,
      })),
  ];
}

/** One line of the list of what a view shows on its cards: a property of the board. */
export interface PropertyToggle {
  id: string;
  template: PropertyTemplate;
}

/**
 * What can be shown or hidden in a view, in the order of the list: the
 * properties of the board. The title is always shown, and so are the marks
 * of a description and of a checklist on a card (card-face/CardFace.svelte),
 * so neither has a switch; a switch nothing reads would only look like one.
 * For the same reason a card relation has a switch in a table only: the face
 * of a card does not show it.
 */
export function propertyToggles(
  templates: readonly PropertyTemplate[],
  viewType: ViewType,
): PropertyToggle[] {
  return templates
    .filter((template) => viewType === 'table' || template.type !== 'relation')
    .map((template) => ({ id: template.id, template }));
}

/** The visible properties with `propertyId` shown or hidden. A newly shown one goes last. */
export function withPropertyVisible(
  shownProperties: readonly string[],
  propertyId: string,
  visible: boolean,
): string[] {
  const others = shownProperties.filter((id) => id !== propertyId);
  if (!visible) return others;
  return shownProperties.includes(propertyId) ? [...shownProperties] : [...others, propertyId];
}
