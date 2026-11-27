/**
 * What a Gantt view works out from its cards: the axis, the rows, the bar of
 * each card, and the arrows between them. The components lay out; this holds what they lay out
 * from, each step worked out again only when what it reads has changed.
 */
import { SvelteSet } from 'svelte/reactivity';

import { barLateSentences } from '$lib/components/properties/relation-labels.ts';
import { m } from '$lib/i18n/paraglide/messages.js';
import { displayProperty } from '$lib/logic/calendar.ts';
import {
  hasVisibleHierarchy,
  hierarchicalRows,
  type HierarchicalRow,
} from '$lib/logic/card-hierarchy.ts';
import type { CalendarDate } from '$lib/logic/dates.ts';
import type { Days, RelationEdge } from '$lib/logic/dependencies.ts';
import type { BoardGroup } from '$lib/logic/group.ts';
import {
  axisOf,
  barsOf,
  cardsBefore,
  dependencyProperty,
  edgeKey,
  groupOfCards,
  onLoops,
  relationsOf,
  rowsOf,
  scaleOf,
  type Axis,
  type BarBox,
  type ChartRelations,
  type ChartRow,
  type RowList,
  type Scale,
} from '$lib/logic/gantt.ts';
import type { GanttZoom } from '$lib/logic/view-members.ts';
import type { BoardView, Card, PropertyColor, PropertyTemplate } from '$lib/logic/wire.ts';
import type { BoardStore } from '$lib/state/index.ts';

import { Placed } from '../calendar/placed.ts';

import { buildArrowLines, type ArrowLine } from './arrow-lines.ts';
import { Kept } from './kept.ts';
import { daysLabel } from './names.ts';

export type { ArrowLine };

export interface ChartSource {
  board: () => BoardStore;
  view: () => BoardView;
  /** The cards the view shows: filtered, searched and sorted. */
  cards: () => readonly Card[];
  /** Today on the reader's calendar. */
  today: () => CalendarDate;
  zoom: () => GanttZoom;
  /** How many days the page has room for. */
  minDays: () => number;
  /** Days the axis shows besides the cards': where a bar is being dragged. */
  reach?: () => Days | undefined;
}

/** The row of a card as it is drawn. */
export interface CardLine {
  kind: 'card';
  id: string;
  /**
   * Where the bar lies across. Not which row it is in: the list puts a row
   * in its place, and a card added above leaves the rows below as they were.
   */
  bar: BarBox;
  /** What the row and the bar show. */
  title: string;
  /** The card's number on its board, which the row shows; none for a template. */
  number?: number;
  /**
   * What a screen reader hears for the bar: the title, the days, the cards
   * it comes after, what is wrong with that (out of order, on a loop), and
   * the group it is in.
   */
  name: string;
  /** The card starts before the day a card it comes after ends. */
  late: boolean;
  /** Following what the card comes after leads back to the card. */
  loop: boolean;
  /** Nesting level in the visible chart (0, 1, or 2). */
  level: number;
  /** Whether this card has visible sub-cards below it in this chart. */
  hasChildren: boolean;
  /** Whether this card's sub-cards are currently collapsed. */
  collapsed: boolean;
  /** Whether the chart displays hierarchy indentation / toggles. */
  showIndent: boolean;
}

/** The row that starts a group. */
export interface GroupLine {
  kind: 'group';
  id: string;
  /** What the group is called: the option, the person, or "No Status". */
  name: string;
  /** The colour name of the option; undefined for a group that is not an option. */
  color: PropertyColor | undefined;
  /** How many cards of the group have a bar. */
  count: number;
}

export type ChartLine = CardLine | GroupLine;

/** How many rows stand in one block of the list (Gantt.svelte). */
const BLOCK = 25;

const PERSON_KINDS: readonly string[] = ['person', 'createdBy', 'updatedBy'];

export class GanttChart {
  readonly #source: ChartSource;
  readonly #collapsedParents = new SvelteSet<string>();

  constructor(source: ChartSource) {
    this.#source = source;
  }

  readonly toggleParent = (cardId: string): void => {
    if (this.#collapsedParents.has(cardId)) this.#collapsedParents.delete(cardId);
    else this.#collapsedParents.add(cardId);
  };

  get #board(): BoardStore {
    return this.#source.board();
  }

  readonly #templates = $derived.by(() => this.#board.board?.cardProperties ?? []);
  readonly #regional = $derived.by(() => this.#board.workspace.regional);

  /** The property the bars are placed by; none: by when the cards were made. */
  readonly template = $derived.by<PropertyTemplate | undefined>(() =>
    displayProperty(this.#source.view().fields.dateProperty, this.#templates),
  );

  /** The days of each card for the reader, and the cards that have none. */
  readonly placed = $derived.by(
    () =>
      new Placed(
        this.#source.cards(),
        this.template,
        this.#regional,
        this.#board.workspace.locale,
        [],
      ),
  );

  /** The axis last worked out. Not reactive. */
  #lastAxis: Axis | undefined;

  /**
   * The days of the chart. The same object while they are the same days: a
   * new axis draws every bar again, and the days a drag asks for (`reach`)
   * often change nothing (the axis is as long as it may be).
   */
  readonly axis = $derived.by<Axis>(() => {
    const reach = this.#source.reach?.();
    const next = axisOf({
      zoom: this.#source.zoom(),
      weekStart: this.#regional.weekStart,
      today: this.#source.today(),
      spans: reach ? [...this.placed.spans, reach] : this.placed.spans,
      minDays: this.#source.minDays(),
    });
    const last = this.#lastAxis;
    if (last?.first === next.first && last.last === next.last && last.zoom === next.zoom) {
      return last;
    }
    this.#lastAxis = next;
    return next;
  });

  readonly scale = $derived.by<Scale>(() => scaleOf(this.axis, this.#regional.weekStart));

  /** The property the view groups by, when the board still has it; as the table reads it. */
  readonly groupBy = $derived.by<PropertyTemplate | undefined>(() => {
    const id = this.#source.view().fields.groupBy;
    return id ? this.#templates.find((template) => template.id === id) : undefined;
  });

  /** The groups of the cards that have days, each in the order of the view; none: not grouped. */
  readonly #groups = $derived.by<BoardGroup[] | undefined>(() => {
    if (!this.groupBy) return undefined;
    const dated = this.placed.spans.flatMap((span) => this.placed.card(span.id) ?? []);
    const groups = this.#board.groupsOf(this.#source.view(), dated, (property) =>
      m['BoardComponent.no-property']({ property }),
    ).visible;
    // A view that names an option twice has it once: a card has one row.
    return groups.filter(
      (group, at) => groups.findIndex((other) => other.option.id === group.option.id) === at,
    );
  });

  /** The cards that have days, grouped parent-before-children with expand/collapse applied. */
  readonly #groupedRows = $derived.by(() => {
    // eslint-disable-next-line svelte/prefer-svelte-reactivity -- rebuilt per derivation, never mutated later
    const byCardId = new Map<string, HierarchicalRow<Card>>();
    let hasHierarchy = false;
    const build = (cards: readonly Card[]): string[] => {
      const rows = hierarchicalRows(cards, this.#collapsedParents);
      if (!hasHierarchy && hasVisibleHierarchy(rows)) hasHierarchy = true;
      for (const row of rows) byCardId.set(row.card.id, row);
      return rows.map((row) => row.card.id);
    };
    const lists: RowList[] = this.#groups
      ? this.#groups.map((group) => ({ groupId: group.option.id, cardIds: build(group.cards) }))
      : [
          {
            cardIds: build(this.placed.spans.flatMap((span) => this.placed.card(span.id) ?? [])),
          },
        ];
    return { lists, byCardId, hasHierarchy };
  });

  /** The cards that have days, as the lists between headings. */
  readonly #lists = $derived.by<RowList[]>(() => this.#groupedRows.lists);

  /** Whether the options of the group by are people, named by the workspace. */
  #person(optionId: string): boolean {
    return optionId !== '' && PERSON_KINDS.includes(this.groupBy?.type ?? '');
  }

  /** The name of a group as the table says it: the option, the person, or "No Status". */
  #groupName(option: BoardGroup['option']): string {
    return this.#person(option.id)
      ? (this.#board.workspace.userName(option.id) ?? m['centerPanel.unknown-user']())
      : option.label;
  }

  /** The name of the group each card is in; none where the view is not grouped. */
  readonly #groupOf = $derived.by(() =>
    groupOfCards(this.#groups ?? [], (group) => this.#groupName(group.option)),
  );

  /** The heading of a group as drawn: named and coloured as the table does. */
  #groupLine(row: Extract<ChartRow, { kind: 'group' }>): GroupLine[] {
    const option = this.#groups?.find((group) => group.option.id === row.id)?.option;
    if (!option || !this.groupBy) return [];
    return [
      {
        kind: 'group',
        id: row.id,
        count: row.count,
        name: this.#groupName(option),
        color: this.#person(option.id) || option.id === '' ? undefined : option.color,
      },
    ];
  }

  readonly rows = $derived.by<ChartRow[]>(() => rowsOf(this.#lists));

  readonly bars = $derived.by(() =>
    barsOf(this.axis, this.rows, (cardId) => this.placed.span(cardId)),
  );

  /** The cards of the rows: a card of a group the view hides has days and no row. */
  readonly #rowCards = $derived.by<Card[]>(() =>
    this.rows.flatMap((row) => (row.kind === 'card' ? (this.placed.card(row.id) ?? []) : [])),
  );

  /** The card relation read as what comes before what; none: no arrows. */
  readonly dependency = $derived.by<PropertyTemplate | undefined>(() =>
    dependencyProperty(this.#source.view().fields.dependencyPropertyId, this.#templates),
  );

  /** The edges between the cards that have a bar. Loops are looked for on the whole board. */
  readonly relations = $derived.by<ChartRelations>(() =>
    relationsOf(this.#rowCards, this.#board.realCards, this.dependency?.id, (cardId) =>
      this.placed.span(cardId),
    ),
  );

  readonly #keptArrows = new Kept<ArrowLine>();

  /** One arrow for each edge. */
  readonly arrows = $derived.by<ArrowLine[]>(() =>
    this.#keptArrows.keep(
      buildArrowLines(this.relations, this.bars, this.axis.width, (cardId) => {
        const card = this.placed.card(cardId);
        return card ? this.placed.title(card) : '';
      }),
      (arrow) => arrow.key,
    ),
  );

  /** For each card, the cards it comes after, in the order of its value. */
  readonly #before = $derived.by(() => cardsBefore(this.relations.edges));

  /** For each card, the edges pointing into it, in edge order. */
  readonly #edgesByTo = $derived.by(() => {
    // eslint-disable-next-line svelte/prefer-svelte-reactivity -- rebuilt per derivation, never mutated later
    const byTo = new Map<string, RelationEdge[]>();
    for (const edge of this.relations.edges) {
      const list = byTo.get(edge.to);
      if (list) list.push(edge);
      else byTo.set(edge.to, [edge]);
    }
    return byTo;
  });

  readonly #listFormat = $derived.by(
    () => new Intl.ListFormat(this.#board.workspace.locale, { type: 'conjunction' }),
  );

  /** The titles of cards as a list in the reader's language: "A, B and C". */
  #listed(cardIds: readonly string[]): string {
    const titles = cardIds.flatMap((id) => {
      const card = this.placed.card(id);
      return card ? [this.placed.title(card)] : [];
    });
    return this.#listFormat.format(titles);
  }

  /**
   * The names worked out so far, by card. A card that changes is a new
   * object, so a name is worked out again only for the card that changed,
   * not for every card of the chart whenever one of them does; everything
   * else a name is made of starts the names over.
   */
  readonly #named = $derived.by(() => ({
    // What a name is made of besides its card: kept here so that the names start over with it.
    of: [this.template, this.#regional, this.#board.workspace.locale, this.#year],
    names: new WeakMap<Card, string>(),
  }));

  /** The year of today: a day in another year is said with its year. */
  readonly #year = $derived.by(() => Number(this.#source.today().slice(0, 4)));

  /** The title of a card (after its time, where its value has one) and its days. */
  #titleAndDays(card: Card): string {
    const { names } = this.#named;
    const known = names.get(card);
    if (known !== undefined) return known;
    const time = this.placed.time(card);
    const title = time
      ? m.calendar_time_title({ time, title: this.placed.saidTitle(card) })
      : this.placed.saidTitle(card);
    const span = this.placed.span(card.id);
    const name = span
      ? m.calendar_card_days({
          title,
          days: daysLabel(span, this.#board.workspace.locale, this.#year, (first, last) =>
            m.gantt_days_range({ first, last }),
          ),
        })
      : title;
    names.set(card, name);
    return name;
  }

  /** The edges pointing into `cardId` whose dates are out of order. */
  #lateEdges(cardId: string): RelationEdge[] {
    const incoming = this.#edgesByTo.get(cardId) ?? [];
    return incoming.filter((edge) => this.relations.late.has(edgeKey(edge)));
  }

  /**
   * What a screen reader hears for a bar: its title and days, the cards it
   * comes after, in words what the dashes and the marks say to the eye, and
   * the group it is in, which the heading says to the eye.
   */
  #name(card: Card): string {
    const before = this.#before.get(card.id);
    const late = barLateSentences(
      this.#lateEdges(card.id),
      (id) => {
        const found = this.placed.card(id);
        return found ? this.placed.title(found) : undefined;
      },
      (titles) => this.#listFormat.format(titles),
    );
    const name = this.#titleAndDays(card);
    const group = this.#groupOf.get(card.id);
    const more = [
      ...late,
      this.relations.loopCards.has(card.id) ? m.gantt_bar_loop() : undefined,
      group === undefined ? undefined : m.gantt_bar_group({ group }),
    ].filter((part) => part !== undefined);
    // The title and the days are a sentence of their own once another follows.
    const first = before
      ? m.gantt_bar_after({ name, cards: this.#listed(before) })
      : more.length > 0
        ? m.gantt_bar_alone({ name })
        : name;
    return [first, ...more].join(' ');
  }

  readonly #keptLines = new Kept<ChartLine>();

  /** The rows as drawn. */
  readonly lines = $derived.by<ChartLine[]>(() =>
    this.#keptLines.keep(this.#linesNow(), (line) => `${line.kind}:${line.id}`),
  );

  /** The rows in blocks, for the list to skip the ones that are out of view. */
  readonly blocks = $derived.by<ChartLine[][]>(() => {
    const blocks: ChartLine[][] = [];
    for (let at = 0; at < this.lines.length; at += BLOCK) {
      blocks.push(this.lines.slice(at, at + BLOCK));
    }
    return blocks;
  });

  #linesNow(): ChartLine[] {
    return this.rows.flatMap((row): ChartLine[] => {
      if (row.kind === 'group') return this.#groupLine(row);
      const card = this.placed.card(row.id);
      const bar = this.bars.get(row.id);
      if (!card || !bar) return [];
      const meta = this.#groupedRows.byCardId.get(row.id);
      return [
        {
          kind: 'card',
          id: row.id,
          bar: { x: bar.x, width: bar.width, fromBefore: bar.fromBefore, toAfter: bar.toAfter },
          title: this.placed.title(card),
          ...(card.number === undefined ? {} : { number: card.number }),
          name: this.#name(card),
          late: this.#lateEdges(row.id).length > 0,
          loop: this.relations.loopCards.has(row.id),
          level: meta?.level ?? 0,
          hasChildren: meta?.hasChildren ?? false,
          collapsed: meta?.collapsed ?? false,
          showIndent: this.#groupedRows.hasHierarchy,
        },
      ];
    });
  }

  /** The rows of cards, from the top: what the arrow keys walk. */
  readonly cardLines = $derived.by(() => this.lines.filter((line) => line.kind === 'card'));

  /** How many cards with a bar are on a loop. */
  readonly loopCount = $derived.by(() => this.cardLines.filter((line) => line.loop).length);

  /** The cards of the view without a date that are on a loop: they have no bar to mark. */
  readonly undatedLoops = $derived.by(() =>
    onLoops(
      this.placed.undated.map((card) => card.id),
      this.relations.loopCards,
    ),
  );
}
