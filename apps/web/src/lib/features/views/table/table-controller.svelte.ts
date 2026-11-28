/**
 * What the table works out from its view, and everything it changes.
 *
 * The component (Table.svelte) lays out; this holds the state it lays out
 * from (columns and their widths, the rows as lists, what is being dragged
 * or edited) and the changes a press leads to. Every change goes through the
 * board store, which shows it at once and can undo it.
 */
import { tick } from 'svelte';
import { SvelteSet } from 'svelte/reactivity';

import type { MenuAction } from '$lib/components/menu/actions.ts';
import { PickerPresentation } from '$lib/components/picker-surface/default-presentation.svelte.ts';
import { readerPointer } from '$lib/components/drag/reader-pointer.ts';
import { Sortable, type SortableMove } from '$lib/drag/sortable.svelte.ts';
import { m } from '$lib/i18n/paraglide/messages.js';
import { moveItems, type ListOrder } from '$lib/logic/drop-target.ts';
import { propertiesThatMeetFilterGroup } from '$lib/logic/filter.ts';
import type { DisplayContext } from '$lib/logic/format.ts';
import type { BoardGroup } from '$lib/logic/group.ts';
import { propertyBehavior, type CalculationOption } from '$lib/logic/properties.ts';
import {
  TITLE_COLUMN_ID,
  type BoardView,
  type Card,
  type PropertyColor,
  type PropertyTemplate,
  type PropertyValue,
} from '$lib/logic/wire.ts';
import type { BoardStore } from '$lib/state/index.ts';

import { activationOf, Selection } from '../selection.svelte.ts';

import { titleProperty } from './calculation-names.ts';
import { CellEditing } from './cell-editing.svelte.ts';
import { ColumnResize } from './column-resize.svelte.ts';
import {
  gridTemplate,
  reorderedIds,
  sortDirection,
  steppedIds,
  storedTitleWidth,
  tableColumns,
  type TableColumn,
} from './columns.ts';
import { COLUMN_DRAG_MESSAGES, rowDragMessages } from './drag-messages.ts';
import { GridFocus } from './grid-focus.ts';
import { isEditableKind, readAsOrder, type RowHost, type TableReader } from './host.ts';
import { headerMenu } from './menus.ts';
import { rowActionsOf, type RowActionsHost } from './row-actions.ts';
import {
  buildTableRowLists,
  listsHaveHierarchy,
  previewMovedRowLists,
  type RowList,
} from './row-lists.ts';
import { ScrollKeeper } from './scroll-keeper.ts';

export type { RowList } from './row-lists.ts';

export interface TableSource {
  board: () => BoardStore;
  view: () => BoardView;
  /** The cards the view shows: filtered, searched and sorted. */
  cards: () => readonly Card[];
  cardHref: (cardId: string) => string;
  openCard: (cardId: string) => void;
}

const COLUMNS = 'columns';
const PERSON_KINDS: readonly string[] = ['person', 'createdBy', 'updatedBy'];

export class TableController {
  readonly #source: TableSource;
  readonly #presentation = new PickerPresentation();

  /** The element that scrolls; the grid. */
  scroller = $state<HTMLElement | undefined>(undefined);
  /** The table is scrolled sideways: the title column casts its shadow. */
  sideways = $state(false);
  /** Said to a screen reader: something that happened without a visible sign of its own. */
  said = $state('');
  /** The groups a reader who may not change the view has folded, for this visit only. */
  #folded = $state.raw<readonly string[]>([]);
  /** Parent cards whose sub-cards are currently collapsed in this table view. */
  readonly #collapsedParents = new SvelteSet<string>();

  readonly grid = new GridFocus();
  readonly editing = new CellEditing((at) => this.grid.find(at.rowId, at.columnId));
  readonly resize = new ColumnResize({
    heading: (columnId) => this.grid.find('header', columnId),
    save: (columnId, width) => {
      const view = this.#view;
      void this.#board.setViewFields(
        view.id,
        { columnWidths: { ...view.fields.columnWidths, [columnId]: width } },
        'resizeColumn',
      );
    },
  });
  readonly keeper = new ScrollKeeper({
    onSideways: (scrolled) => {
      this.sideways = scrolled;
    },
  });

  constructor(source: TableSource) {
    this.#source = source;
  }

  get #board(): BoardStore {
    return this.#source.board();
  }

  get #view(): BoardView {
    return this.#source.view();
  }

  readonly canEdit = $derived.by(() => this.#board.canEdit);
  readonly #templates = $derived.by(() => this.#board.board?.cardProperties ?? []);

  // ------------------------------------------------------------ columns

  /** Stored column metadata: unaffected by live drag pixel width so rows/calculations stay untouched (§1.2.2). */
  readonly columns = $derived.by<TableColumn[]>(() => tableColumns(this.#view, this.#templates));
  /** Stored width of the title column; undefined leaves it to the pane. */
  readonly titleWidth = $derived.by(() => storedTitleWidth(this.#view.fields.columnWidths));
  /** Live or stored width of the title column for `--table-title` and its `HeaderCell`. */
  readonly liveTitleWidth = $derived.by(() =>
    this.resize.live?.id === TITLE_COLUMN_ID ? this.resize.live.width : this.titleWidth,
  );
  readonly template = $derived.by(() => {
    const live = this.resize.live;
    return gridTemplate(
      this.columns.map((column) => (live?.id === column.id ? live.width : column.width)),
    );
  });
  /** The title column, for the summary row: decoupled from live drag width (§1.2.2). */
  readonly titleColumn = $derived.by<TableColumn>(() => ({
    id: TITLE_COLUMN_ID,
    template: titleProperty(m['TableComponent.name']()),
    width: this.titleWidth ?? 0,
  }));

  /** The headings in the order they stand in: while the keyboard carries one, where it would land. */
  readonly headerColumns = $derived.by<TableColumn[]>(() => {
    const drag = this.columnSort.active;
    if (drag?.mode !== 'keyboard' || !drag.gap) return this.columns;
    const ids = moveItems(this.#columnOrder, drag.itemIds, drag.gap)[0]?.items ?? [];
    return ids.flatMap((id) => this.columns.find((column) => column.id === id) ?? []);
  });

  readonly #columnOrder = $derived.by<ListOrder[]>(() => [
    { id: COLUMNS, items: this.columns.map((column) => column.id) },
  ]);

  /** Where a column dragged by a pointer would land: before or after which heading. */
  readonly columnMarker = $derived.by<{ id: string; side: 'before' | 'after' } | undefined>(() => {
    const drag = this.columnSort.active;
    if (drag?.mode !== 'pointer' || !drag.gap || drag.outside) return undefined;
    const rest = this.columns.map((column) => column.id).filter((id) => !drag.itemIds.includes(id));
    const before = rest[drag.gap.index];
    if (before !== undefined) return { id: before, side: 'before' };
    const last = rest.at(-1);
    return last === undefined ? undefined : { id: last, side: 'after' };
  });

  #setVisible(ids: string[] | undefined, description: string): void {
    if (ids) void this.#board.setViewFields(this.#view.id, { shownProperties: ids }, description);
  }

  /** The lines of the menu of a heading. */
  headerActions(id: string): MenuAction[] {
    const view = this.#view;
    const shown = this.columns.map((column) => column.id);
    const visible = view.fields.shownProperties;
    return headerMenu({
      sorted: sortDirection(view.fields.sort, id),
      place: id === TITLE_COLUMN_ID ? undefined : { at: shown.indexOf(id), count: shown.length },
      width: this.columns.find((column) => column.id === id)?.width,
      onSort: (reversed) => {
        void this.#board.setViewFields(
          view.id,
          { sort: [{ propertyId: id, reversed }] },
          'sortColumn',
        );
      },
      onHide: () => {
        this.#setVisible(
          visible.filter((other) => other !== id),
          'hideColumn',
        );
      },
      onMove: (by) => {
        this.#setVisible(steppedIds(visible, shown, id, by), 'moveColumn');
      },
      onResize: (delta) => {
        this.resize.by(id, delta);
      },
    });
  }

  readonly columnSort = new Sortable({
    scope: 'column',
    axis: 'x',
    pointer: readerPointer,
    order: () => this.#columnOrder,
    onDrop: ({ itemIds, to }) => {
      if (to.kind !== 'index') return;
      const shown = moveItems(this.#columnOrder, itemIds, to)[0]?.items ?? [];
      this.#setVisible(reorderedIds(this.#view.fields.shownProperties, shown), 'moveColumn');
    },
    messages: COLUMN_DRAG_MESSAGES,
    itemLabel: (id) => this.columns.find((column) => column.id === id)?.template.name ?? id,
  });

  // --------------------------------------------------------------- rows

  /** The property the view groups by, when the board still has it. */
  readonly groupBy = $derived.by<PropertyTemplate | undefined>(() => {
    const id = this.#view.fields.groupBy;
    return id ? this.#templates.find((template) => template.id === id) : undefined;
  });
  /** The group of a card is not a value that can be set: who made it, who changed it last. */
  readonly #groupFixed = $derived.by(
    () => this.groupBy !== undefined && propertyBehavior(this.groupBy.type).isReadOnly,
  );
  readonly #collapsed = $derived.by<readonly string[]>(() =>
    this.canEdit ? this.#view.fields.collapsedGroups : this.#folded,
  );

  /** The rows as lists, grouped by parent-child hierarchy. */
  readonly lists = $derived.by<RowList[]>(() => {
    const cards = this.#source.cards();
    const groups = this.groupBy
      ? this.#board.groupsOf(this.#view, cards, (property) =>
          m['BoardComponent.no-property']({ property }),
        ).visible
      : undefined;
    return buildTableRowLists(cards, groups, this.#collapsed, this.#collapsedParents);
  });

  readonly #rowOrder = $derived.by<ListOrder[]>(() =>
    this.lists.map((list) => ({ id: list.id, items: list.cards.map((card) => card.id) })),
  );

  /** The rows as drawn: while the keyboard carries one, it stands where it would land. */
  readonly shownLists = $derived.by<RowList[]>(() => {
    const drag = this.rows.active;
    if (drag?.mode !== 'keyboard' || !drag.gap) return this.lists;
    return previewMovedRowLists(this.lists, this.#rowOrder, drag.itemIds, drag.gap);
  });

  /** Whether any visible row in the table has parent-child nesting. */
  readonly hasHierarchy = $derived.by(() => listsHaveHierarchy(this.shownLists));

  toggleParent(cardId: string): void {
    if (this.#collapsedParents.has(cardId)) this.#collapsedParents.delete(cardId);
    else this.#collapsedParents.add(cardId);
  }

  resetCollapsed(): void {
    this.#collapsedParents.clear();
  }

  /** Rows can be moved by hand: the view has no sort, and the reader may change it. */
  readonly canDragRows = $derived.by(() => this.canEdit && this.#view.fields.sort.length === 0);

  /**
   * Where a row dragged by a pointer would land: just above the row
   * `before`, or at the end of the list `endOf`.
   */
  readonly rowMarker = $derived.by<{ before?: string; endOf?: string } | undefined>(() => {
    const drag = this.rows.active;
    const gap = drag?.gap;
    if (drag?.mode !== 'pointer' || !gap || drag.outside) return undefined;
    const list = this.#rowOrder.find((candidate) => candidate.id === gap.listId);
    if (!list) return undefined;
    const before = list.items.filter((id) => !drag.itemIds.includes(id))[gap.index];
    return before === undefined ? { endOf: list.id } : { before };
  });

  readonly rows = new Sortable({
    order: () => this.#rowOrder,
    // The reader's setting decides whether a row a finger picked up buzzes.
    pointer: readerPointer,
    onDrop: (move) => {
      this.#dropRows(move);
    },
    messages: rowDragMessages((listId) => {
      const group = this.lists.find((list) => list.id === listId)?.group;
      return group ? this.groupName(group) : undefined;
    }),
    itemLabel: (id) => this.#board.card(id)?.title || m['KanbanCard.untitled'](),
  });

  #dropRows({ itemIds, from, to }: SortableMove): void {
    if (to.kind !== 'index') return;
    const list = this.lists.find((candidate) => candidate.id === to.listId);
    if (!list) return;
    // Who made a card cannot be changed by dropping it among another person's.
    if (this.#groupFixed && from.listId !== to.listId) return;
    void this.#board.dropCards({
      viewId: this.#view.id,
      cardIds: itemIds,
      optionId: list.group && !this.#groupFixed ? list.group.option.id : undefined,
      // A place among the rows the list shows: the store counts the hidden ones in.
      index: to.index,
      shown: list.cards.map((card) => card.id),
    });
  }

  /** What a group is called: its option, the person, or "No Status". */
  groupName(group: BoardGroup): string {
    if (group.option.id === '' || !this.groupBy) return group.option.label;
    if (!PERSON_KINDS.includes(this.groupBy.type)) return group.option.label;
    return this.#board.workspace.userName(group.option.id) ?? m['centerPanel.unknown-user']();
  }

  /** The colour of a group's chip; undefined for a group that is not an option. */
  groupColor(group: BoardGroup): PropertyColor | undefined {
    if (group.option.id === '' || !this.groupBy) return undefined;
    return PERSON_KINDS.includes(this.groupBy.type) ? undefined : group.option.color;
  }

  /** Folds or unfolds a group. For a reader who may not change the view it lasts for this visit. */
  toggleGroup(list: RowList): void {
    const id = list.group?.option.id;
    if (id === undefined) return;
    const collapsed = this.#collapsed;
    const next = collapsed.includes(id)
      ? collapsed.filter((other) => other !== id)
      : [...collapsed, id];
    if (this.canEdit) {
      void this.#board.setViewFields(this.#view.id, { collapsedGroups: next }, 'foldGroup');
    } else {
      this.#folded = next;
    }
  }

  /** Whether a list ends with "+ New", and its group heading has a "+". */
  canAddTo(list: RowList): boolean {
    return this.canEdit && !(list.group && this.#groupFixed);
  }

  /** "+ New" after all groups, where no group can be given a new card. */
  readonly canAddLoose = $derived.by(() => this.canEdit && this.#groupFixed);

  /**
   * Makes a card the view will show (it gets the values the view's filter
   * asks for, and the value of the group it was made in) and puts the caret
   * in its title.
   */
  async addCard(list: RowList | undefined): Promise<void> {
    const board = this.#board;
    const view = this.#view;
    if (!board.board) return;
    const properties: Record<string, PropertyValue> = propertiesThatMeetFilterGroup(
      view.fields.filter,
      board.board.cardProperties,
    );
    const groupBy = this.groupBy;
    if (list?.group && groupBy && !this.#groupFixed) {
      const optionId = list.group.option.id;
      if (optionId === '') {
        // The group of cards without a value: the filter's value for it would move the card elsewhere.
        properties[groupBy.id] = '';
      } else {
        properties[groupBy.id] = optionId;
      }
      if (list.collapsed) this.toggleGroup(list);
    }
    const draft = Object.fromEntries(
      Object.entries(properties).filter(([, value]) => value !== ''),
    );
    const card = await board.createCard({ properties: draft });
    if (card) {
      await tick();
      this.editing.start(card.id, TITLE_COLUMN_ID);
    }
  }

  // -------------------------------------------------------------- cells

  /** What the calculations show dates and times by. */
  readonly context = $derived.by<DisplayContext>(() => {
    const { locale, regional } = this.#board.workspace;
    return { locale, timeZone: regional.timeZone, hourCycle: regional.hourCycle };
  });

  readonly #reader = $derived.by<TableReader>(() => {
    const workspace = this.#board.workspace;
    return {
      locale: workspace.locale,
      regional: workspace.regional,
      users: workspace.users,
      people: this.#board.people,
      presentation: this.#presentation.current,
    };
  });

  chooseCalculation(columnId: string, option: CalculationOption): void {
    const view = this.#view;
    void this.#board.setViewFields(
      view.id,
      { columnCalculations: { ...view.fields.columnCalculations, [columnId]: option } },
      'changeCalculation',
    );
  }

  /** The rows selected, to change together; in the order the table shows them. */
  readonly selection = new Selection(() =>
    this.shownLists.flatMap((list) => list.rows.map((row) => row.card.id)),
  );

  readonly #rowActions: RowActionsHost = {
    board: () => this.#board,
    cards: () => this.#source.cards(),
    cardHref: (cardId) => this.#source.cardHref(cardId),
    openCard: (cardId) => {
      this.#source.openCard(cardId);
    },
    grid: this.grid,
    selection: this.selection,
    say: (text) => {
      this.#say(text);
    },
  };

  #say(text: string): void {
    // The same words twice in a row would not be read again.
    this.said = '';
    void tick().then(() => {
      this.said = text;
    });
  }

  /** What the rows are given. One object for all of them, made once. */
  readonly host: RowHost = ((table: TableController): RowHost => ({
    get reader() {
      return table.#reader;
    },
    get canEdit() {
      return table.canEdit;
    },
    get cards() {
      return table.#board.cardsById;
    },
    ordering: (id) => readAsOrder(table.#board, id),
    cardHref: (cardId) => table.#source.cardHref(cardId),
    open: (cardId) => {
      table.#source.openCard(cardId);
    },
    activate: (card, template) => {
      if (!table.canEdit || !isEditableKind(template.type)) return;
      if (template.type === 'checkbox') {
        const checked = card.fields.properties[template.id] === 'true';
        void table.#board.setCardProperty(card.id, template.id, checked ? '' : 'true');
      } else {
        table.editing.start(card.id, template.id);
      }
    },
    editTitle: (cardId) => {
      table.editing.start(cardId, TITLE_COLUMN_ID);
    },
    setValue: (cardId, propertyId, value) => {
      void table.#board.setCardProperty(cardId, propertyId, value);
    },
    setTitle: (cardId, title) => {
      void table.#board.setCardTitle(cardId, title);
    },
    toggleParent: (cardId) => {
      table.toggleParent(cardId);
    },
    rowActions: (card) => rowActionsOf(table.#rowActions, card),
    pick: (cardId, event) => {
      const how = activationOf(event);
      if (!table.canEdit || how === undefined || !table.selection.press(cardId, how)) return false;
      // Shift and a press also select the text between; the rows are what was meant.
      if (how === 'range') document.getSelection()?.removeAllRanges();
      return true;
    },
    toggleSelected: (cardId, range) => {
      table.selection.press(cardId, range ? 'range' : 'toggle');
    },
  }))(this);
}
