/**
 * What the table gives each of its rows, and the classes its parts share.
 *
 * A row gets one object instead of a dozen callbacks: the table makes it
 * once, and a row that is drawn anew does not get new functions.
 */
import type { MenuAction } from '$lib/components/menu/actions.ts';
import type { Presentation } from '$lib/components/picker-surface/presentation.ts';
import type { PropertyUser } from '$lib/components/properties/index.ts';
import { orderingRelations } from '$lib/logic/gantt.ts';
import type { Regional } from '$lib/logic/regional.ts';
import type { Card, PropertyTemplate, PropertyType, PropertyValue } from '$lib/logic/wire.ts';
import type { BoardStore } from '$lib/state/index.ts';

/** How values are shown and edited for the reader. */
export interface TableReader {
  locale: string;
  regional: Regional;
  /** Everybody a card may name, to show a name by. */
  users: readonly PropertyUser[];
  /** Who a person property can be set to: the members of the board. */
  people: readonly PropertyUser[];
  /** How pickers open: a sheet on a compact page, a popover otherwise. */
  presentation: Presentation;
}

export interface RowHost {
  readonly reader: TableReader;
  /** The reader may change cards. */
  readonly canEdit: boolean;
  /** The board's cards that are there, by identifier: what a relation names. */
  readonly cards: ReadonlyMap<string, Card>;
  /** A Gantt view reads the relation as an order: no card that closes a loop is offered. */
  ordering(propertyId: string): boolean;
  cardHref(cardId: string): string;
  open: (cardId: string) => void;
  /** A cell was pressed (or Enter on it): edit it, or for a checkbox change it. */
  activate(card: Card, template: PropertyTemplate): void;
  /** The title cell was pressed. */
  editTitle(cardId: string): void;
  setValue(cardId: string, propertyId: string, value: PropertyValue | undefined): void;
  setTitle(cardId: string, title: string): void;
  /** Expands or collapses a parent card's visible sub-cards in the table. */
  toggleParent(cardId: string): void;
  /** The lines of the "more" menu of a row. */
  rowActions(card: Card): MenuAction[];
  /**
   * A press on a row with Ctrl or ⌘ or Shift, or any press while selecting:
   * the row goes into the selection or out of it. Says whether it did.
   */
  pick(cardId: string, event: MouseEvent): boolean;
  /** The box of a row: takes the row in or out, or with Shift the range up to it. */
  toggleSelected(cardId: string, range: boolean): void;
}

/** The kinds of property a cell of the table can change. */
const EDITABLE: ReadonlySet<PropertyType> = new Set<PropertyType>([
  'text',
  'number',
  'email',
  'phone',
  'url',
  'select',
  'multiSelect',
  'date',
  'person',
  'multiPerson',
  'checkbox',
  'relation',
]);

export function isEditableKind(type: PropertyType): boolean {
  return EDITABLE.has(type);
}

/** The row every row is: the tracks come from the table, so that one variable resizes them all. */
export const ROW = 'grid grid-cols-(--table-columns)';

/**
 * A row of a card that is out of view is not laid out or painted until it
 * comes near: with hundreds of rows that is most of the work of drawing the
 * table and of scrolling it. Until it has been drawn once it is taken to be
 * as high as a row of one line.
 */
export const ROW_SKIPPED =
  '[contain-intrinsic-block-size:auto_2.25rem] [content-visibility:auto] roomy:[contain-intrinsic-block-size:auto_2.75rem]';

/**
 * The "more" button of a row: an icon button of 28 pixels, 44 in the roomy
 * density. A plain element with these classes and not the shared icon
 * button, which costs a component per row: there may be hundreds.
 */
export const MORE_BUTTON =
  'inline-flex size-7 shrink-0 items-center justify-center rounded-lg text-foreground outline-none select-none hover:bg-muted focus-visible:ring-3 focus-visible:ring-ring/50 aria-expanded:bg-muted roomy:size-touch';

/** The least height of a row: a mouse's, and a finger's in the roomy density. */
export const ROW_HEIGHT = 'min-h-9 roomy:min-h-touch';

/**
 * A cell of the title column. It stays at the left edge while the other
 * columns pass under it, and once they do, it casts a shadow to its right
 * (the table says so with `data-scrolled`).
 */
export const PINNED =
  'sticky start-0 z-10 bg-background after:pointer-events-none after:absolute after:inset-y-0 after:start-full after:w-2 after:bg-linear-to-r after:from-(--scrim) after:to-transparent after:opacity-0 after:transition-opacity group-data-scrolled/table:after:opacity-100 rtl:after:bg-linear-to-l';

/** The focus of a cell is drawn inside it: its neighbours would cover a ring around it. */
export const CELL_FOCUS = 'outline-ring focus-visible:outline-2 focus-visible:-outline-offset-2';

/** A line across the top of a row: where the dragged row would land. */
export const DROP_LINE_ROW =
  'before:pointer-events-none before:absolute before:inset-x-0 before:top-0 before:z-20 before:h-0.5 before:-translate-y-1/2 before:bg-ring';

/** Whether a Gantt view of the board reads the relation `propertyId` as an order (`orderingRelations`). */
export function readAsOrder(board: BoardStore, propertyId: string): boolean {
  const views = board.views.map((view) => view.fields);
  return orderingRelations(views, board.board?.cardProperties ?? []).has(propertyId);
}
